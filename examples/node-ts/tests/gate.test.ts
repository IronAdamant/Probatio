import assert from "node:assert/strict"
import test from "node:test"
import { gate } from "../src/gate.ts"

test("zero stays shut", () => {
  assert.equal(gate(0), false)
})
