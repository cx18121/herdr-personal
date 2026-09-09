import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import type { ExecOptions, ExecResult } from "@earendil-works/pi-coding-agent";
import { mkdir, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createJumpTool, executeJump } from "./index";

type Exec = (command: string, args: string[], options?: ExecOptions) => Promise<ExecResult>;

type Call = {
	command: string;
	args: string[];
	options?: ExecOptions;
};

const originalPaneId = process.env.HERDR_PANE_ID;
let testRoot: string;

function success(stdout = ""): ExecResult {
	return { stdout, stderr: "", code: 0, killed: false };
}

function json(result: unknown): ExecResult {
	return success(JSON.stringify({ id: "test", result }));
}

beforeEach(async () => {
	process.env.HERDR_PANE_ID = "w1:p1";
	testRoot = `/var/tmp/pi-herdr-worktree-jump-${crypto.randomUUID()}`;
	await mkdir(testRoot, { recursive: true });
});

afterEach(async () => {
	if (originalPaneId === undefined) {
		delete process.env.HERDR_PANE_ID;
	} else {
		process.env.HERDR_PANE_ID = originalPaneId;
	}
	await rm(testRoot, { recursive: true, force: true });
});

describe("pi-herdr-worktree-jump", () => {
	test("describes the Worktrunk lifecycle and local default branch", () => {
		const tool = createJumpTool({ exec: async () => success() });

		expect(tool.description).toContain("Worktrunk creates and initializes");
		expect(tool.promptGuidelines?.join("\n")).toContain("only when the user explicitly asks");
		expect(tool.promptGuidelines?.join("\n")).toContain("remind the user");
		expect(tool.promptGuidelines?.join("\n")).toContain("keep it current with its upstream");
	});

	test("creates through Worktrunk, opens through Herdr, and starts the replacement Pi", async () => {
		const sourceCheckout = `${testRoot}/source`;
		const sourceSubdirectory = `${sourceCheckout}/src/nested`;
		const worktreePath = `${testRoot}/worktrees/issue-2325`;
		const currentSession = `${testRoot}/current.jsonl`;
		const newSession = `${testRoot}/new.jsonl`;
		await mkdir(sourceSubdirectory, { recursive: true });
		await mkdir(worktreePath, { recursive: true });
		const canonicalSource = await realpath(sourceCheckout);
		const canonicalWorktree = await realpath(worktreePath);
		await writeFile(currentSession, '{"type":"session","cwd":"source"}\n');
		await writeFile(
			newSession,
			`${JSON.stringify({ type: "session", cwd: worktreePath, parentSession: currentSession })}\n`,
		);

		const calls: Call[] = [];
		const exec: Exec = async (command, args, options) => {
			calls.push({ command, args, options });
			if (command === "sh") return success();
			if (command === "wt") {
				return success(JSON.stringify({ action: "created", path: worktreePath, branch: "issue/2325-jump" }));
			}
			if (args[0] === "worktree" && args[1] === "list") {
				return json({ source: { source_checkout_path: sourceCheckout }, worktrees: [] });
			}
			if (args[0] === "worktree" && args[1] === "open") {
				return json({
					workspace: { workspace_id: "w2" },
					tab: { tab_id: "w2:t1" },
					root_pane: { pane_id: "w2:p1" },
					worktree: { path: worktreePath, branch: "issue/2325-jump" },
				});
			}
			if (args[0] === "pane" && args[1] === "run") return success();
			throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
		};

		const statuses: Array<[string, string | undefined]> = [];
		const notifications: string[] = [];
		let shutdown = false;
		const result = await executeJump(
			{ exec },
			{
				cwd: sourceSubdirectory,
				sessionManager: { getSessionFile: () => currentSession },
				ui: {
					setStatus(name, value) {
						statuses.push([name, value]);
					},
					notify(message) {
						notifications.push(message);
					},
				},
				shutdown() {
					shutdown = true;
				},
			},
			undefined,
			() => ({ getSessionFile: () => newSession }),
			{ destination: "new", branch: "issue/2325-jump", label: "issue 2325" },
		);

		expect(calls[0]).toMatchObject({
			command: "herdr",
			args: ["worktree", "list", "--cwd", sourceSubdirectory],
			options: { cwd: sourceSubdirectory, timeout: 10_000 },
		});
		expect(calls[1]).toMatchObject({
			command: "wt",
			args: ["switch", "--create", "issue/2325-jump", "--no-cd", "--format=json"],
			options: { cwd: canonicalSource, timeout: 600_000 },
		});
		expect(calls[2]).toMatchObject({
			command: "herdr",
			args: [
				"worktree",
				"open",
				"--cwd",
				canonicalSource,
				"--path",
				worktreePath,
				"--label",
				"issue 2325",
				"--focus",
			],
		});
		const paneRun = calls.find((call) => call.command === "herdr" && call.args[0] === "pane");
		expect(paneRun?.args.slice(0, 3)).toEqual(["pane", "run", "w2:p1"]);
		expect(paneRun?.args[3]).toContain(`'pi' '--session' '${newSession}'`);

		const header = JSON.parse((await readFile(newSession, "utf8")).split("\n")[0]);
		expect(header.parentSession).toBeUndefined();
		expect(statuses.at(-1)).toEqual(["herdr-worktree-jump", undefined]);
		expect(notifications.at(-1)).toContain(worktreePath);
		expect(shutdown).toBe(true);
		expect(result.terminate).toBe(true);
		expect(result.details).toMatchObject({
			destination: "new",
			worktreePath: canonicalWorktree,
			branch: "issue/2325-jump",
			workspaceId: "w2",
			paneId: "w2:p1",
		});
	});

	test("generates a branch name and passes an explicit base to Worktrunk", async () => {
		const sourceCheckout = `${testRoot}/source`;
		const worktreePath = `${testRoot}/worktree`;
		const currentSession = `${testRoot}/current.jsonl`;
		const newSession = `${testRoot}/new.jsonl`;
		await mkdir(sourceCheckout, { recursive: true });
		await mkdir(worktreePath, { recursive: true });
		await writeFile(currentSession, '{"type":"session","cwd":"source"}\n');
		await writeFile(newSession, '{"type":"session","cwd":"worktree"}\n');

		const calls: Call[] = [];
		let generatedBranch = "";
		const exec: Exec = async (command, args, options) => {
			calls.push({ command, args, options });
			if (command === "sh") return success();
			if (command === "wt") {
				generatedBranch = args[2] ?? "";
				return success(JSON.stringify({ path: worktreePath, branch: generatedBranch }));
			}
			if (args[0] === "worktree" && args[1] === "list") {
				return json({ source: { source_checkout_path: sourceCheckout } });
			}
			if (args[0] === "worktree" && args[1] === "open") {
				return json({
					workspace: { workspace_id: "w2" },
					root_pane: { pane_id: "w2:p1" },
					worktree: { path: worktreePath, branch: generatedBranch },
				});
			}
			if (args[0] === "pane") return success();
			throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
		};

		await executeJump(
			{ exec },
			{
				cwd: sourceCheckout,
				sessionManager: { getSessionFile: () => currentSession },
				ui: { setStatus() {}, notify() {} },
				shutdown() {},
			},
			undefined,
			() => ({ getSessionFile: () => newSession }),
			{ destination: "new", base: "@" },
		);

		expect(generatedBranch).toMatch(/^worktree\/pi-[a-z0-9]+-[a-f0-9]{4}$/);
		expect(calls[1]?.args).toEqual([
			"switch",
			"--create",
			generatedBranch,
			"--no-cd",
			"--format=json",
			"--base",
			"@",
		]);
	});

	test("reports a worktree left behind by a failed Worktrunk hook", async () => {
		const sourceCheckout = `${testRoot}/source`;
		const worktreePath = `${testRoot}/worktree`;
		const currentSession = `${testRoot}/current.jsonl`;
		await mkdir(sourceCheckout, { recursive: true });
		await writeFile(currentSession, '{"type":"session","cwd":"source"}\n');

		const exec: Exec = async (command, args) => {
			if (command === "wt") {
				return { stdout: "", stderr: "pre-start hook failed", code: 1, killed: false };
			}
			if (command === "git") {
				return success(`worktree ${worktreePath}\nHEAD deadbeef\nbranch refs/heads/feature/hook-failure\n`);
			}
			if (args[0] === "worktree" && args[1] === "list") {
				return json({ source: { source_checkout_path: sourceCheckout } });
			}
			throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
		};

		await expect(
			executeJump(
				{ exec },
				{
					cwd: sourceCheckout,
					sessionManager: { getSessionFile: () => currentSession },
					ui: { setStatus() {}, notify() {} },
					shutdown() {},
				},
				undefined,
				() => ({ getSessionFile: () => undefined }),
				{ destination: "new", branch: "feature/hook-failure" },
			),
		).rejects.toThrow(
			`Worktrunk failed after creating feature/hook-failure at ${worktreePath}. ` +
				"Remove it with wt remove --foreground 'feature/hook-failure' before retrying.\n\n" +
				"pre-start hook failed",
		);
	});

	test("reports a checkout left behind when Herdr cannot open it", async () => {
		const sourceCheckout = `${testRoot}/source`;
		const worktreePath = `${testRoot}/worktree`;
		const currentSession = `${testRoot}/current.jsonl`;
		await mkdir(sourceCheckout, { recursive: true });
		await writeFile(currentSession, '{"type":"session","cwd":"source"}\n');

		const exec: Exec = async (command, args) => {
			if (command === "wt") {
				return success(JSON.stringify({ path: worktreePath, branch: "feature/open-failure" }));
			}
			if (args[0] === "worktree" && args[1] === "list") {
				return json({ source: { source_checkout_path: sourceCheckout } });
			}
			if (args[0] === "worktree" && args[1] === "open") {
				return success(JSON.stringify({ error: { code: "open_failed", message: "could not open" } }));
			}
			throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
		};

		await expect(
			executeJump(
				{ exec },
				{
					cwd: sourceCheckout,
					sessionManager: { getSessionFile: () => currentSession },
					ui: { setStatus() {}, notify() {} },
					shutdown() {},
				},
				undefined,
				() => ({ getSessionFile: () => undefined }),
				{ destination: "new", branch: "feature/open-failure" },
			),
		).rejects.toThrow(
			`Worktrunk created feature/open-failure at ${worktreePath}, but Herdr could not open it. ` +
				"The checkout remains available. Remove it with wt remove --foreground 'feature/open-failure' if it is not needed.\n\n" +
				"open_failed: could not open",
		);
	});

	test("opens the main checkout without invoking Worktrunk", async () => {
		const sourceCheckout = `${testRoot}/source`;
		const linkedCheckout = `${testRoot}/worktrees/linked`;
		const currentSession = `${testRoot}/current.jsonl`;
		const newSession = `${testRoot}/new.jsonl`;
		await mkdir(sourceCheckout, { recursive: true });
		await mkdir(linkedCheckout, { recursive: true });
		const canonicalSource = await realpath(sourceCheckout);
		await writeFile(currentSession, '{"type":"session","cwd":"linked"}\n');
		await writeFile(newSession, '{"type":"session","cwd":"source"}\n');

		const calls: Call[] = [];
		const exec: Exec = async (command, args, options) => {
			calls.push({ command, args, options });
			if (command === "sh") return success();
			if (args[0] === "worktree" && args[1] === "list") {
				return json({ source: { source_checkout_path: sourceCheckout } });
			}
			if (args[0] === "worktree" && args[1] === "open") {
				return json({
					workspace: { workspace_id: "w-main" },
					root_pane: { pane_id: "w-main:p-occupied" },
					worktree: { path: sourceCheckout, branch: "master" },
					already_open: true,
				});
			}
			if (args[0] === "tab" && args[1] === "create") {
				return json({ tab: { tab_id: "w-main:t-new" }, root_pane: { pane_id: "w-main:p-new" } });
			}
			if (args[0] === "pane") return success();
			throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
		};

		await executeJump(
			{ exec },
			{
				cwd: linkedCheckout,
				sessionManager: { getSessionFile: () => currentSession },
				ui: { setStatus() {}, notify() {} },
				shutdown() {},
			},
			undefined,
			() => ({ getSessionFile: () => newSession }),
			{ destination: "main" },
		);

		expect(calls.some((call) => call.command === "wt")).toBe(false);
		expect(calls[1]?.args).toEqual([
			"worktree",
			"open",
			"--cwd",
			canonicalSource,
			"--path",
			canonicalSource,
			"--focus",
		]);
		expect(calls[2]?.args).toEqual([
			"tab",
			"create",
			"--workspace",
			"w-main",
			"--cwd",
			canonicalSource,
			"--focus",
		]);
	});

	test("refuses to jump when the current session is not persisted", async () => {
		await expect(
			executeJump(
				{ exec: async () => success() },
				{
					cwd: testRoot,
					sessionManager: { getSessionFile: () => undefined },
					ui: { setStatus() {}, notify() {} },
					shutdown() {},
				},
				undefined,
				() => ({ getSessionFile: () => undefined }),
				{ destination: "new" },
			),
		).rejects.toThrow("not persisted");
	});
});
