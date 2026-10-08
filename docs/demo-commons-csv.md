# Demo: gaps, new tests, then a hidden bug (Commons CSV, 2026-10-08)

The claim to test: an agent asks Probatio where a file's tests are blind, writes one test per gap, and a real bug that later comes back is caught by those new tests. This page is the attempt, run blind, and its result. **The result is negative.** No hidden bug was caught by the new tests. It is written down because a demo that only reports wins is not evidence.

## Protocol

- Repository: Apache Commons CSV (Apache-2.0), a clone with full history. No Probatio author wrote its tests.
- Subjects were chosen by a script that printed only commit hashes, dates, and file names: fix commits that add one new test file and change one source file. The commit subject, the diff, and the new test were not read until the end.
- For each subject, the fixed tree is the starting point. The bug is the fix's source diff reversed (`git diff -R <fix>^ <fix> -- src/main`), written to a patch file and not displayed. The new test file is the revealing test, and it is hidden.
- **Before:** `mutate sealed` with the bug patch and `--hide <new test>`. Only the counts were printed, never `next` or `gaps`, which name the line.
- **Gap pass:** in a workspace where the revealing test does not exist, `mutate generate --src <the touched file> --operators wide`, then `mutate run`. One test per survivor, written from the code as it stands, as AGENTS.md says. An equivalent survivor is recorded with `findings add`, not tested.
- **After:** the same sealed run on the workspace with the new tests, with `--fix <fix commit>` so the fix-edited check reads the fix's diff, not the test commit's.

## Subject 1: `a4c6037d` (CSV-265), `ExtendedBufferedReader.java`

- **Before:** baseline green, 419 tests. `0 no coverage, 1 survived, 0 killed`. The existing suite runs the line and does not catch the bug.
- **Gap pass:** 52 wide mutants. `8 no coverage, 3 survived, 39 killed, 1 timed out, 1 errored`. The survivors were all in `read(char[], int, int)`:
  - `len > 0` to `len >= 0`. Equivalent: with `length > 0`, `BufferedReader.read` returns at least 1 or -1, never 0. Recorded with `findings add --status equivalent`.
  - `len == -1` to `len == -0`, and `len == -1` to `len != -1`. At end of stream `lastChar` would no longer become `END_OF_STREAM`. `ExtendedBufferedReaderGapTest` (below) reads to the end through `read(char[])` and checks `getLastChar()`. Rerun with confirm on: both killed, each by both new tests.
- **After:** `0 no coverage, 1 survived, 0 killed`. A miss.
- **Why, read after the run:** the fix rewrote `readLine()` so it goes through `read()`, which counts `position` and keeps the real line ending in `lastChar`. The old `readLine()` called `super.readLine()` and did neither. Every operator mutant inside `readLine()` was already killed by existing tests. Nothing asserts `getPosition()` after `readLine()`, and no operator mutant says "this update never happens".
- **Post-hoc, not blind:** after the run, wide mode gained statement deletion for semicolon languages. Run on the same workspace, it adds 12 deletions. 3 survive: `lastChar = END_OF_STREAM` in `close()` and in `read(char[])`, and `position += len` in `read(char[])`. The last one is the class of gap the bug lives in (position is not asserted), one method away from the bug. That is a pointer, not a catch, and it is not counted as one.

## Subject 2: `3eac15fc` (CSV-211), `CSVFormat.java`

Ineligible. The fixed tree does not build on this toolchain: `maven-bundle-plugin` 4.2.1 fails with an internal `ConcurrentModificationException`. The rules say not to patch a project to make it pass, so the run is a refusal.

## Subject 3: `42ded1cf`, `Token.java`

- **Before:** baseline green, 905 tests. `0 no coverage, 1 survived, 0 killed`.
- **Gap pass:** 7 wide mutants, 0 survivors, 4 on lines no test runs (`toString()` and a buffer capacity). Three of the four were `+` in a string concatenation, which led to a fix: wide text operators no longer turn `"x" + y` into `"x" - y`. `TokenGapTest` covers `toString()` and `reset()`.
- **After:** `0 no coverage, 1 survived, 0 killed`.
- **Why:** this commit is not a bug fix. It renamed a constant, edited comments, and changed `type + " ["` to `type.name() + " ["`. For an enum without its own `toString()` those are the same string. The reverted "bug" is equivalent, so no test can catch it. The candidate filter (adds a test, changes one source file) cannot tell a refactor from a fix.

## What this says

- The loop works as a gap finder. On subject 1 it found two real gaps that 419 tests left open, and one test closed both.
- On these subjects it did not reach the hidden bugs. One was a whole-method rewrite whose missing side effects no operator mutant expresses. The other was not a bug.
- A blind demo of the whole loop still needs a subject where the bug is the kind of change operators model (a condition, a boundary, a dropped update), on a tree that builds. Commons CSV has no further candidate that adds a new test file and changes one source file. The next search is another Apache Commons library, or BugsInPy, under the same protocol.

## The tests written in the gap pass

`src/test/java/org/apache/commons/csv/ExtendedBufferedReaderGapTest.java`:

```java
@Test
public void testReadIntoArrayMarksTheEndOfStream() throws Exception {
    try (ExtendedBufferedReader reader = new ExtendedBufferedReader(new StringReader("ab"))) {
        final char[] buf = new char[10];
        assertEquals(2, reader.read(buf, 0, buf.length));
        assertEquals('b', reader.getLastChar());
        assertEquals(-1, reader.read(buf, 0, buf.length));
        assertEquals(END_OF_STREAM, reader.getLastChar());
    }
}

@Test
public void testReadIntoArrayAtOffsetKeepsTheLastCharRead() throws Exception {
    try (ExtendedBufferedReader reader = new ExtendedBufferedReader(new StringReader("xyz"))) {
        final char[] buf = new char[8];
        assertEquals(3, reader.read(buf, 4, 4));
        assertEquals('z', reader.getLastChar());
        assertEquals('x', buf[4]);
        assertEquals(END_OF_STREAM, reader.read(buf, 0, buf.length));
        assertEquals(END_OF_STREAM, reader.getLastChar());
    }
}
```

`src/test/java/org/apache/commons/csv/TokenGapTest.java` asserts `"INVALID []"` for a new token, `"TOKEN [a,b]"` after setting type and content, and that `reset()` clears type, content, `isReady`, and `isQuoted`.

## Commands

```bash
probatio mutate sealed --package <fixed clone> --repo <fixed clone> --patches <dir with the reversed fix> \
  --label <label file outside the clone> --hide <new test file> --out <fresh dir> --workers 1 --fix <fix commit>
probatio mutate generate --package <workspace> --src <touched file> --out <gen> --operators wide
probatio mutate run --package <workspace> --patches <gen>/mutants --out <run> --workers 1 --no-confirm
probatio findings add --state <state> --id <mutant> --status equivalent --reason "<why>"
```
