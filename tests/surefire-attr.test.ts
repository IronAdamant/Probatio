import assert from "node:assert/strict"
import test from "node:test"
import { surefireAttr } from "../src/mutate/suites.ts"

test("surefire name is not the classname prefix", () => {
  const classnameFirst = `classname="gate.GateTest" name="testShut[0]" time="0.004">`
  assert.equal(surefireAttr(classnameFirst, "name"), "testShut[0]")
  assert.equal(surefireAttr(classnameFirst, "classname"), "gate.GateTest")

  const nameFirst = `name="testShut[0]" classname="gate.GateTest" time="0.004">`
  assert.equal(surefireAttr(nameFirst, "name"), "testShut[0]")
  assert.equal(surefireAttr(nameFirst, "classname"), "gate.GateTest")

  const afterTag = `classname="gate.GateTest" name="testShut" time="0.004"><failure>name="other"`
  assert.equal(surefireAttr(afterTag, "name"), "testShut")
  assert.equal(surefireAttr(`classname="gate.GateTest">`, "name"), "")
})
