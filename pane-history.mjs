#!/usr/bin/env node

import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const configDir = path.join(os.homedir(), ".config", "herdr");
const statePath = path.join(configDir, "closed-panes.json");
const lockPath = path.join(configDir, "closed-panes.lock");
const herdr = process.env.HERDR_BIN_PATH ?? "herdr";

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

const hasErrorCode = (error, code) =>
  typeof error === "object" && error !== null && "code" in error && error.code === code;

const withHistoryLock = async (callback) => {
  await mkdir(configDir, { recursive: true });
  let acquired = false;

  for (let attempt = 0; attempt < 2400; attempt++) {
    try {
      await mkdir(lockPath);
      acquired = true;
      break;
    } catch (error) {
      if (!hasErrorCode(error, "EEXIST")) throw error;
      await delay(50);
    }
  }

  if (!acquired) throw new Error("Timed out waiting for the pane history lock.");

  try {
    return await callback();
  } finally {
    await rm(lockPath, { recursive: true, force: true });
  }
};

const runHerdr = (args) => {
  const result = spawnSync(herdr, args, { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || result.stdout.trim() || `herdr ${args.join(" ")} failed`);
  }
  return result.stdout.trim() ? JSON.parse(result.stdout) : undefined;
};

const readHistory = async () => {
  try {
    const value = JSON.parse(await readFile(statePath, "utf8"));
    return Array.isArray(value) ? value : [];
  } catch {
    return [];
  }
};

const writeHistory = async (history) => {
  await mkdir(configDir, { recursive: true });
  const temporaryPath = `${statePath}.${process.pid}.tmp`;
  await writeFile(temporaryPath, `${JSON.stringify(history.slice(-20), null, 2)}\n`, "utf8");
  await rename(temporaryPath, statePath);
};

const activePaneRecord = () => {
  const paneId = process.env.HERDR_ACTIVE_PANE_ID;
  if (!paneId) {
    throw new Error("Herdr did not provide the active pane identity.");
  }

  const pane = runHerdr(["pane", "get", paneId]).result.pane;
  const tab = runHerdr(["tab", "get", pane.tab_id]).result.tab;
  const piSession = pane.agent === "pi" && pane.agent_session?.kind === "path" &&
    existsSync(pane.agent_session.value)
    ? pane.agent_session.value
    : undefined;

  return {
    cwd: pane.foreground_cwd || pane.cwd || process.env.HERDR_ACTIVE_PANE_CWD || os.homedir(),
    piSession,
    tabId: pane.tab_id,
    tabLabel: tab.label,
    workspaceId: pane.workspace_id,
  };
};

const closeActivePane = async () => {
  const paneId = process.env.HERDR_ACTIVE_PANE_ID;
  if (!paneId) {
    throw new Error("Herdr did not provide the active pane identity.");
  }

  await withHistoryLock(async () => {
    const history = await readHistory();
    history.push(activePaneRecord());
    await writeHistory(history);

    try {
      runHerdr(["pane", "close", paneId]);
    } catch (error) {
      history.pop();
      await writeHistory(history);
      throw error;
    }
  });
};

const isPaneBusy = (result) => {
  const output = `${result.stderr}\n${result.stdout}`;
  return output.includes("agent_pane_busy");
};

const restorePi = async (paneId, sessionFile) => {
  const args = [
    "agent",
    "start",
    `reopened-${randomUUID().slice(0, 8)}`,
    "--kind",
    "pi",
    "--pane",
    paneId,
    "--timeout",
    "10000",
    "--",
    "--session",
    sessionFile,
  ];

  for (let attempt = 0; attempt < 10; attempt++) {
    const result = spawnSync(herdr, args, { encoding: "utf8" });
    if (result.status === 0) return;
    if (!isPaneBusy(result) || attempt === 9) {
      throw new Error(result.stderr.trim() || result.stdout.trim() || "Could not restore Pi session.");
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
};

const createPane = (record) => {
  const workspaces = runHerdr(["workspace", "list"]).result.workspaces;
  const workspaceId = workspaces.some((workspace) => workspace.workspace_id === record.workspaceId)
    ? record.workspaceId
    : process.env.HERDR_ACTIVE_WORKSPACE_ID;
  if (!workspaceId) {
    throw new Error("There is no workspace available for the reopened pane.");
  }

  const cwd = existsSync(record.cwd)
    ? record.cwd
    : process.env.HERDR_ACTIVE_PANE_CWD || os.homedir();
  const tabs = runHerdr(["tab", "list"]).result.tabs;
  const tab = tabs.find((candidate) => candidate.tab_id === record.tabId);
  if (!tab) {
    return runHerdr([
      "tab",
      "create",
      "--workspace",
      workspaceId,
      "--cwd",
      cwd,
      "--label",
      record.tabLabel || path.basename(cwd),
      "--focus",
    ]).result.root_pane;
  }

  runHerdr(["tab", "focus", tab.tab_id]);
  const panes = runHerdr(["pane", "list"]).result.panes
    .filter((pane) => pane.tab_id === tab.tab_id);
  const target = panes.find((pane) => pane.focused) ?? panes[0];
  if (!target) {
    throw new Error(`Tab ${tab.tab_id} has no pane to split.`);
  }

  return runHerdr([
    "pane",
    "split",
    "--pane",
    target.pane_id,
    "--direction",
    "right",
    "--cwd",
    cwd,
    "--focus",
  ]).result.pane;
};

const reopenLastPane = async () => {
  await withHistoryLock(async () => {
    const history = await readHistory();
    const record = history.at(-1);
    if (!record) return;

    const pane = createPane(record);
    try {
      if (record.piSession && existsSync(record.piSession)) {
        await restorePi(pane.pane_id, record.piSession);
      }
      history.pop();
      await writeHistory(history);
    } catch (error) {
      spawnSync(herdr, ["pane", "close", pane.pane_id], { encoding: "utf8" });
      throw error;
    }
  });
};

const action = process.argv[2];
if (action === "close") {
  await closeActivePane();
} else if (action === "reopen") {
  await reopenLastPane();
} else {
  throw new Error("Usage: pane-history.mjs <close|reopen>");
}
