import assert from "node:assert/strict"
import test from "node:test"
import { nodeTooOld } from "../src/node-version.ts"

test("node older than 22.6 is named, and 22.6 is accepted", () => {
  assert.match(nodeTooOld("20.18.0") ?? "", /22\.6/)
  assert.match(nodeTooOld("22.5.1") ?? "", /22\.5\.1/)
  assert.equal(nodeTooOld("22.6.0"), null)
  assert.equal(nodeTooOld("24.1.0"), null)
  assert.match(nodeTooOld("not-a-version") ?? "", /22\.6/)
})
