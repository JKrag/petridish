/**
 * Reads ~/.petridish/projects.json. Read-only, same file every petridish
 * frontend (swab list, petri) consumes — never write to this path from here.
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { Radar } from "../types.ts";

export const DEFAULT_STATE_PATH = path.join(
  os.homedir(),
  ".petridish",
  "projects.json",
);

/** Same missing-file case swab/petri report — same wording, on purpose. */
export class StateFileMissingError extends Error {}

export function readRadar(statePath: string = DEFAULT_STATE_PATH): Radar {
  if (!fs.existsSync(statePath)) {
    throw new StateFileMissingError(
      `no state file at ${statePath}; run 'swab scan' first`,
    );
  }
  const text = fs.readFileSync(statePath, "utf-8");
  return JSON.parse(text) as Radar;
}
