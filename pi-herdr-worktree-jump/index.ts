import { defineTool, SessionManager, type ExtensionAPI, type ExtensionContext } from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { Type, type Static, type TSchema } from "typebox";
import { Value } from "typebox/value";

const HerdrErrorSchema = Type.Object({
	code: Type.Optional(Type.String()),
	message: Type.Optional(Type.String()),
});

const HerdrEnvelopeSchema = Type.Object(
	{
		result: Type.Optional(Type.Unknown()),
		error: Type.Optional(HerdrErrorSchema),
	},
	{ additionalProperties: true },
);

const WorktreeListSchema = Type.Object(
	{
		source: Type.Optional(
			Type.Object({
				source_checkout_path: Type.Optional(Type.String()),
			}),
		),
	},
	{ additionalProperties: true },
);

const WorktreeOpenedSchema = Type.Object(
	{
		workspace: Type.Optional(Type.Object({ workspace_id: Type.Optional(Type.String()) })),
		tab: Type.Optional(Type.Object({ tab_id: Type.Optional(Type.String()) })),
		root_pane: Type.Optional(Type.Object({ pane_id: Type.Optional(Type.String()) })),
		worktree: Type.Optional(
			Type.Object({
				path: Type.Optional(Type.String()),
				branch: Type.Optional(Type.String()),
			}),
		),
		already_open: Type.Optional(Type.Boolean()),
	},
	{ additionalProperties: true },
);

const WorktrunkSwitchSchema = Type.Object(
	{
		path: Type.Optional(Type.String()),
		branch: Type.Optional(Type.String()),
	},
	{ additionalProperties: true },
);

const TabCreatedSchema = Type.Object(
	{
		tab: Type.Optional(Type.Object({ tab_id: Type.Optional(Type.String()) })),
		root_pane: Type.Optional(Type.Object({ pane_id: Type.Optional(Type.String()) })),
	},
	{ additionalProperties: true },
);

const SessionHeaderSchema = Type.Object(
	{
		parentSession: Type.Optional(Type.Unknown()),
	},
	{ additionalProperties: true },
);

type JumpDestination = "new" | "main";

type JumpOptions = {
	destination: JumpDestination;
	branch?: string;
	base?: string;
	label?: string;
};

type JumpTarget = {
	worktreePath: string;
	branch?: string;
	workspaceId?: string;
	tabId?: string;
	rootPaneId: string;
};

type CommandRunner = Pick<ExtensionAPI, "exec">;

type JumpContext = {
	cwd: string;
	sessionManager: Pick<ExtensionContext["sessionManager"], "getSessionFile">;
	ui: Pick<ExtensionContext["ui"], "setStatus" | "notify">;
	shutdown: () => void;
};

type SessionForker = (
	currentFile: string,
	worktreePath: string,
) => Pick<SessionManager, "getSessionFile">;

export default function (pi: ExtensionAPI) {
	if (process.env.HERDR_ENV !== "1") return;

	pi.registerTool(createJumpTool(pi));
}

export function createJumpTool(pi: CommandRunner) {
	return defineTool({
		name: "herdr_worktree_jump",
		label: "Herdr Worktree Jump",
		description:
			"Relocate this Pi session either into a new Worktrunk-managed linked Git worktree or back to the repository's main checkout. Worktrunk creates and initializes the checkout before Herdr opens the destination workspace. The replacement Pi starts in a dedicated Herdr pane, then the old Pi shuts down and its pane closes. This is an explicit session relocation, not a general isolation or worktree-planning tool.",
		promptSnippet: "Jump this Pi session to a new worktree or back to the main checkout only when explicitly requested",
		promptGuidelines: [
			"Use herdr_worktree_jump only when the user explicitly asks to jump or move this Pi session into a new worktree or back to the repository's main checkout.",
			"Before herdr_worktree_jump creates a worktree without an explicit base, remind the user that Worktrunk uses the local default branch and that they should keep it current with its upstream.",
			"Never use herdr_worktree_jump merely because isolation would be useful, repository instructions recommend a worktree, or the task appears non-trivial.",
		],
		parameters: Type.Object({
			destination: Type.Optional(
				StringEnum(["new", "main"] as const, {
					description: "Use new to create a worktree, or main to return to the repository's main checkout. Defaults to new.",
				}),
			),
			branch: Type.Optional(
				Type.String({
					description: "Branch name for destination=new. If omitted, the tool generates one.",
				}),
			),
			base: Type.Optional(
				Type.String({
					description:
						"Git ref used as the new branch base for destination=new. If omitted, Worktrunk uses the repository's local default branch.",
				}),
			),
			label: Type.Optional(
				Type.String({
					description: "Optional Herdr workspace label for destination=new.",
				}),
			),
		}),
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return executeJump(pi, ctx, signal, SessionManager.forkFrom, {
				destination: params.destination ?? "new",
				branch: cleanOptional(params.branch),
				base: cleanOptional(params.base),
				label: cleanOptional(params.label),
			});
		},
	});
}

export async function executeJump(
	pi: CommandRunner,
	ctx: JumpContext,
	signal: AbortSignal | undefined,
	forkFrom: SessionForker,
	options: JumpOptions,
) {
	const oldPaneId = process.env.HERDR_PANE_ID;
	if (!oldPaneId) {
		throw new Error("HERDR_PANE_ID is missing; cannot close the old Herdr pane safely");
	}

	const currentFile = ctx.sessionManager.getSessionFile();
	if (!currentFile) {
		throw new Error("Current Pi session is not persisted, so it cannot jump to a worktree");
	}
	if (options.destination === "main" && (options.branch || options.base || options.label)) {
		throw new Error("branch, base, and label apply only when destination is new");
	}

	ctx.ui.setStatus("herdr-worktree-jump", "resolving repository");
	let newSessionFile: string | undefined;
	let replacementStarted = false;

	try {
		const sourceCheckout = await resolveSourceCheckout(pi, signal, ctx.cwd);
		let target: JumpTarget;
		if (options.destination === "main") {
			const currentDirectory = await canonicalDirectory(ctx.cwd);
			if (isWithinDirectory(sourceCheckout, currentDirectory)) {
				throw new Error(`Pi is already inside the main checkout: ${sourceCheckout}`);
			}
			ctx.ui.setStatus("herdr-worktree-jump", "opening main checkout");
			target = await openMainCheckout(pi, signal, sourceCheckout);
		} else {
			ctx.ui.setStatus("herdr-worktree-jump", "creating worktree");
			target = await createWorktree(pi, signal, sourceCheckout, options);
		}
		const worktreePath = await canonicalDirectory(target.worktreePath);

		newSessionFile = await forkSessionFile(currentFile, worktreePath, forkFrom);
		await runInNewPane(pi, signal, target.rootPaneId, newSessionFile, worktreePath);
		replacementStarted = true;

		let cleanupWarning: string | undefined;
		try {
			await scheduleOldPaneCleanup(pi, currentFile, oldPaneId, process.pid);
		} catch (error) {
			cleanupWarning = error instanceof Error ? error.message : String(error);
		}

		ctx.ui.setStatus("herdr-worktree-jump", undefined);
		const destinationLabel = options.destination === "main" ? "main checkout" : "worktree";
		ctx.ui.notify(`Jumped Pi session to Herdr ${destinationLabel}: ${worktreePath}`, "info");
		ctx.shutdown();

		const warning = cleanupWarning
			? `\n\nWarning: the old pane could not be scheduled for automatic cleanup: ${cleanupWarning}`
			: "";

		return {
			content: [
				{
					type: "text" as const,
					text:
						`Started replacement Pi in Herdr ${destinationLabel}: ${worktreePath}\n` +
						`Workspace: ${target.workspaceId ?? "unknown"}\n` +
						`Pane: ${target.rootPaneId}\n` +
						`Branch: ${target.branch ?? (options.destination === "new" ? "generated" : "detached")}\n\n` +
						"The old Pi process is shutting down. Its pane will close after it exits." +
						warning,
				},
			],
			details: {
				destination: options.destination,
				worktreePath,
				branch: target.branch,
				workspaceId: target.workspaceId,
				tabId: target.tabId,
				paneId: target.rootPaneId,
				newSessionFile,
				oldSessionFile: currentFile,
				oldPaneId,
				cleanupWarning,
			},
			terminate: true,
		};
	} catch (error) {
		ctx.ui.setStatus("herdr-worktree-jump", undefined);
		if (newSessionFile && !replacementStarted) {
			await rm(newSessionFile, { force: true }).catch(() => undefined);
		}
		throw error;
	}
}

async function resolveSourceCheckout(
	pi: CommandRunner,
	signal: AbortSignal | undefined,
	cwd: string,
): Promise<string> {
	const result = await herdrResult(
		pi,
		["worktree", "list", "--cwd", cwd],
		WorktreeListSchema,
		signal,
		cwd,
		10_000,
	);
	const sourcePath = result?.source?.source_checkout_path;
	if (!sourcePath) {
		throw new Error("Herdr worktree list response did not include source.source_checkout_path");
	}
	return canonicalDirectory(sourcePath);
}

async function createWorktree(
	pi: CommandRunner,
	signal: AbortSignal | undefined,
	sourceCheckout: string,
	options: JumpOptions,
): Promise<JumpTarget> {
	const branch = options.branch ?? generateBranchName();
	const switchArgs = ["switch", "--create", branch, "--no-cd", "--format=json"];
	if (options.base) switchArgs.push("--base", options.base);

	let switched: Static<typeof WorktrunkSwitchSchema>;
	try {
		switched = await worktrunkJson(pi, switchArgs, signal, sourceCheckout, 600_000);
	} catch (error) {
		const leftoverPath = await findWorktreePath(pi, signal, sourceCheckout, branch);
		if (!leftoverPath) throw error;

		const reason = error instanceof Error ? error.message : String(error);
		throw new Error(
			`Worktrunk failed after creating ${branch} at ${leftoverPath}. ` +
				`Remove it with wt remove --foreground ${shellQuote(branch)} before retrying.\n\n${reason}`,
		);
	}
	if (!switched.path) {
		throw new Error("Worktrunk switch response did not include path");
	}

	const openArgs = [
		"worktree",
		"open",
		"--cwd",
		sourceCheckout,
		"--path",
		switched.path,
		"--label",
		options.label ?? switched.branch ?? branch,
		"--focus",
	];
	let opened: Static<typeof WorktreeOpenedSchema> | undefined;
	try {
		opened = await herdrResult(pi, openArgs, WorktreeOpenedSchema, signal, sourceCheckout, 30_000);
	} catch (error) {
		const reason = error instanceof Error ? error.message : String(error);
		throw new Error(
			`Worktrunk created ${branch} at ${switched.path}, but Herdr could not open it. ` +
				`The checkout remains available. Remove it with wt remove --foreground ${shellQuote(branch)} if it is not needed.\n\n${reason}`,
		);
	}
	const worktreePath = opened?.worktree?.path ?? switched.path;
	const rootPaneId = opened?.root_pane?.pane_id;
	if (!rootPaneId) {
		throw new Error(`Herdr opened no root pane for the Worktrunk checkout: ${worktreePath}`);
	}

	return {
		worktreePath,
		rootPaneId,
		branch: opened?.worktree?.branch ?? switched.branch ?? branch,
		workspaceId: opened?.workspace?.workspace_id,
		tabId: opened?.tab?.tab_id,
	};
}

async function openMainCheckout(
	pi: CommandRunner,
	signal: AbortSignal | undefined,
	sourceCheckout: string,
): Promise<JumpTarget> {
	const opened = await herdrResult(
		pi,
		["worktree", "open", "--cwd", sourceCheckout, "--path", sourceCheckout, "--focus"],
		WorktreeOpenedSchema,
		signal,
		sourceCheckout,
		30_000,
	);
	const workspaceId = opened?.workspace?.workspace_id;
	if (!workspaceId) {
		throw new Error("Herdr worktree open response did not include workspace.workspace_id");
	}

	if (opened?.already_open) {
		const tab = await herdrResult(
			pi,
			["tab", "create", "--workspace", workspaceId, "--cwd", sourceCheckout, "--focus"],
			TabCreatedSchema,
			signal,
			sourceCheckout,
			10_000,
		);
		const tabId = tab?.tab?.tab_id;
		const rootPaneId = tab?.root_pane?.pane_id;
		if (!tabId || !rootPaneId) {
			throw new Error("Herdr tab create response did not include tab.tab_id and root_pane.pane_id");
		}
		return {
			worktreePath: sourceCheckout,
			rootPaneId,
			branch: opened.worktree?.branch,
			workspaceId,
			tabId,
		};
	}

	const worktreePath = opened?.worktree?.path ?? sourceCheckout;
	const rootPaneId = opened?.root_pane?.pane_id;
	if (!rootPaneId) {
		throw new Error("Herdr worktree open response did not include root_pane.pane_id");
	}
	return {
		worktreePath,
		rootPaneId,
		branch: opened?.worktree?.branch,
		workspaceId,
		tabId: opened?.tab?.tab_id,
	};
}

async function runInNewPane(
	pi: CommandRunner,
	signal: AbortSignal | undefined,
	paneId: string,
	sessionFile: string,
	worktreePath: string,
): Promise<void> {
	const continuation = `Moved to worktree ${worktreePath}. Continue.`;
	const command = ["pi", "--session", sessionFile, continuation].map(shellQuote).join(" ");
	await herdr(pi, ["pane", "run", paneId, command], signal, worktreePath, 10_000);
}

async function scheduleOldPaneCleanup(
	pi: CommandRunner,
	oldSessionFile: string,
	oldPaneId: string,
	oldPid: number,
): Promise<void> {
	const cleanup = [
		`old_pid=${oldPid}`,
		`old_session=${shellQuote(oldSessionFile)}`,
		`old_pane=${shellQuote(oldPaneId)}`,
		"i=0",
		"while kill -0 \"$old_pid\" 2>/dev/null && [ \"$i\" -lt 600 ]; do i=$((i + 1)); sleep 0.1; done",
		"rm -f -- \"$old_session\"",
		"herdr pane close \"$old_pane\" >/dev/null 2>&1 || true",
	].join("; ");

	const launcher =
		"if command -v setsid >/dev/null 2>&1; then " +
		`setsid sh -c ${shellQuote(cleanup)} >/dev/null 2>&1 < /dev/null & ` +
		"else " +
		`nohup sh -c ${shellQuote(cleanup)} >/dev/null 2>&1 < /dev/null & ` +
		"fi";

	const result = await pi.exec("sh", ["-lc", launcher], { timeout: 5_000 });
	if (result.code !== 0) {
		throw new Error(result.stderr || result.stdout || "failed to launch cleanup process");
	}
}

async function forkSessionFile(
	currentFile: string,
	worktreePath: string,
	forkFrom: SessionForker,
): Promise<string> {
	const forked = forkFrom(currentFile, worktreePath);
	const newFile = forked.getSessionFile();
	if (!newFile) {
		throw new Error("Failed to create forked Pi session file for the new worktree");
	}

	const raw = await readFile(newFile, "utf8");
	const lines = raw.trimEnd().split("\n");
	if (lines[0]) {
		const header = Value.Parse(SessionHeaderSchema, JSON.parse(lines[0]));
		if (header.parentSession !== undefined) {
			delete header.parentSession;
			lines[0] = JSON.stringify(header);
			await writeFile(newFile, `${lines.join("\n")}\n`, "utf8");
		}
	}

	return newFile;
}

async function canonicalDirectory(path: string): Promise<string> {
	const resolved = resolve(path.replace(/^@/, ""));
	const info = await stat(resolved).catch(() => undefined);
	if (!info?.isDirectory()) {
		throw new Error(`Directory does not exist: ${resolved}`);
	}
	return realpath(resolved);
}

function isWithinDirectory(parent: string, child: string): boolean {
	const pathFromParent = relative(parent, child);
	return (
		pathFromParent === "" ||
		(pathFromParent !== ".." && !pathFromParent.startsWith(`..${sep}`) && !isAbsolute(pathFromParent))
	);
}

async function findWorktreePath(
	pi: CommandRunner,
	signal: AbortSignal | undefined,
	sourceCheckout: string,
	branch: string,
): Promise<string | undefined> {
	const result = await pi.exec(
		"git",
		["-C", sourceCheckout, "worktree", "list", "--porcelain"],
		{ cwd: sourceCheckout, signal, timeout: 10_000 },
	);
	if (signal?.aborted || result.killed || result.code !== 0) return undefined;

	const expectedBranch = `branch refs/heads/${branch}`;
	for (const record of result.stdout.trim().split(/\n\s*\n/)) {
		const lines = record.split("\n");
		if (!lines.includes(expectedBranch)) continue;

		const worktree = lines.find((line) => line.startsWith("worktree "));
		if (worktree) return worktree.slice("worktree ".length);
	}
	return undefined;
}

async function worktrunkJson(
	pi: CommandRunner,
	args: string[],
	signal: AbortSignal | undefined,
	cwd: string,
	timeout: number,
): Promise<Static<typeof WorktrunkSwitchSchema>> {
	const result = await pi.exec("wt", args, { cwd, signal, timeout });
	if (signal?.aborted || result.killed) throw new Error("Aborted");
	if (result.code !== 0) {
		throw new Error(result.stderr.trim() || result.stdout.trim() || `wt ${args.join(" ")} failed`);
	}

	const raw = result.stdout.trim();
	try {
		return Value.Parse(WorktrunkSwitchSchema, JSON.parse(raw));
	} catch {
		throw new Error(`Worktrunk returned invalid JSON for ${args.join(" ")}: ${raw}`);
	}
}

async function herdrResult<T extends TSchema>(
	pi: CommandRunner,
	args: string[],
	resultSchema: T,
	signal: AbortSignal | undefined,
	cwd: string,
	timeout: number,
): Promise<Static<T> | undefined> {
	const result = await herdr(pi, args, signal, cwd, timeout);
	const raw = result.stdout.trim() || result.stderr.trim();
	let response: Static<typeof HerdrEnvelopeSchema>;
	try {
		response = Value.Parse(HerdrEnvelopeSchema, JSON.parse(raw));
	} catch {
		throw new Error(`Herdr returned invalid JSON for ${args.join(" ")}: ${raw}`);
	}
	if (response.error) {
		throw new Error(`${response.error.code ?? "herdr_error"}: ${response.error.message ?? "unknown Herdr error"}`);
	}
	if (response.result === undefined) return undefined;

	try {
		return Value.Parse(resultSchema, response.result);
	} catch {
		throw new Error(`Herdr returned an invalid result for ${args.join(" ")}: ${raw}`);
	}
}

async function herdr(
	pi: CommandRunner,
	args: string[],
	signal: AbortSignal | undefined,
	cwd: string,
	timeout: number,
) {
	const result = await pi.exec("herdr", args, { cwd, signal, timeout });
	if (signal?.aborted || result.killed) throw new Error("Aborted");
	if (result.code !== 0) {
		throw new Error(parseHerdrFailure(result.stderr, result.stdout, args));
	}
	return result;
}

function parseHerdrFailure(stderr: string, stdout: string, args: string[]): string {
	for (const output of [stderr, stdout]) {
		const trimmed = output.trim();
		if (!trimmed) continue;
		try {
			const response = Value.Parse(HerdrEnvelopeSchema, JSON.parse(trimmed));
			if (response.error) {
				return `${response.error.code ?? "herdr_error"}: ${response.error.message ?? "unknown Herdr error"}`;
			}
		} catch {
			return trimmed;
		}
	}
	return `herdr ${args.join(" ")} failed`;
}

function generateBranchName(): string {
	return `worktree/pi-${Date.now().toString(36)}-${crypto.randomUUID().slice(0, 4)}`;
}

function cleanOptional(value: unknown): string | undefined {
	if (typeof value !== "string") return undefined;
	const trimmed = value.trim();
	return trimmed || undefined;
}

function shellQuote(value: string): string {
	return `'${value.replace(/'/g, `'"'"'`)}'`;
}
