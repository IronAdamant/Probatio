import assert from "node:assert/strict"
import test from "node:test"
import { selectionFor, type CoverageMap } from "../src/mutate/coverage-map.ts"

test("a repeated file name does not steal another file's coverage", () => {
  const map: CoverageMap = {
    files: {
      "src/span/simple.py": { "12": ["tests/test_a.py::test_a"] },
      "src/span_handlers/simple.py": { "32": ["tests/test_b.py::test_b"] },
    },
  }
  const hit = selectionFor(map, [{ file: "llama-index-instrumentation/src/span_handlers/simple.py", line: 32 }])
  assert.equal(hit.state, "covered")
  if (hit.state === "covered") assert.deepEqual(hit.tests, ["tests/test_b.py::test_b"])
  const miss = selectionFor(map, [{ file: "llama-index-instrumentation/src/span/simple.py", line: 32 }])
  assert.equal(miss.state, "uncovered")
})
