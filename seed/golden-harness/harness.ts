// Shared by golden.test.ts and the case files in golden/cases.
import { homedir } from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"

/** The package root and home folder on the machine running the table (recorded rows use these). */
export const PKG = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..")
export const HOME = homedir()

/** A golden row: a call, or a call pinned to the instant it was recorded at. */
export type GoldenRow = (() => unknown) | { now: number; run: () => unknown }
export type GoldenCases = Record<string, GoldenRow>

/** Rows without their own instant run at this one. */
export const GOLDEN_NOW = Date.parse("2026-09-30T00:00:00.000Z")

/** Pin a row to the clock it was recorded at, so expiry and age rules read the same every run. */
export function at(now: number, run: () => unknown): GoldenRow {
  return { now, run }
}
