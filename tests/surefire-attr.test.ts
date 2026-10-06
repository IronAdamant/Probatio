import assert from "node:assert/strict"
import test from "node:test"
import { surefireArg, surefireAttr } from "../src/mutate/suites.ts"

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

test("surefire joins methods of one class with a plus", () => {
  assert.equal(surefireArg(["gate.GateTest#testOpens", "gate.GateTest#testShut"]), "gate.GateTest#testOpens+testShut")
  assert.equal(surefireArg(["a.A#one", "b.B#two", "a.A#three"]), "a.A#one+three,b.B#two")
  assert.equal(surefireArg(["gate.GateTest#testShut*"]), "gate.GateTest#testShut*")
  assert.equal(surefireArg(["gate.GateTest"]), "gate.GateTest")
  assert.equal(surefireArg([]), "")
})
