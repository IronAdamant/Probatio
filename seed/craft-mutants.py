# Hand-made mutants: each puts one historical bug back into today's src (b38e85f).
import subprocess, sys, os
WT = sys.argv[1]; OUT = sys.argv[2]
SRC = os.path.join(WT, "examples/auspex-ts/src")
M = {
 "0ecb8e6": [("sandbox.ts", "  if (onSignIn) {", "  if (false && onSignIn) {")],
 "69792bf": [("assert_receipt.py", 'SKIP_TAGS = ("script", "style", "noscript", "title", "template")', 'SKIP_TAGS = ("script", "style", "noscript")')],
 "0248893": [("profile-status.ts", "  if (result.botWall) {", "  if (false && result.botWall) {")],
 "992f512": [("session-ledger.ts", "  const run = queue.then(step, step)", "  const run = step()")],
 "4907a9b": [("http-url.ts", "COUNTRY_SECOND_LEVEL.has(second) ? 3 : 2", "second.length <= 3 ? 3 : 2"),
             ("profile-persist.ts", " || (d.includes(\".\") && hostIs(app, d))", "")],
 "473da1e": [("page-actions.ts", "  const reason = [...lines].reverse().find(", "  const reason = undefined && [...lines].reverse().find(")],
 "5174688": [("paths.ts", "  if (inHome) return `~/", "  if (false && inHome) return `~/")],
 "7b85b59": [("check.ts", "      if (landedOnForbiddenHost(landedAt) || (landedAt !== opts.url && (await resolvesToForbiddenHost(landedAt)))) {", "      if (false) {"),
             ("assert_receipt.py", "        if forbidden_host(url) or resolves_forbidden(url):", "        if False:")],
 "af5a0c2": [("job.ts", "      record.nextCall = checkVerifyNextCall(record.profile, target)", "      record.nextCall = resumeJobNextCall(record.jobId, record.profile)"),
             ("job.ts", "      record.nextCall = newJobNextCall(record.profile, target)", "      record.nextCall = resumeJobNextCall(record.jobId, record.profile)")],
 "b0deeed": [("save-drain.ts", "      if (pid === 0 && (await lockIsYoung(file))) return false", "      if (false && pid === 0 && (await lockIsYoung(file))) return false")],
 "ef2f3a0": [("errors.ts", "    error: stripAnsi(issue.message),", "    error: issue.message,")],
 "eff9dfa": [("operator-session.ts", "Number.isFinite(row.lastUsedMs) ? row.lastUsedMs : nowMs", "Number.isFinite(row.lastUsedMs) ? row.lastUsedMs : 0")],
 "f979056": [("page-actions.ts", "await clearField(box, timeout, signal)", "void 0", "all")],
 "faea669": [("cli.ts", "  if (url === undefined) return\n  if (!isHttpOrHttpsUrl(url)) fail(\"url must be an http or https URL\")\n  if (!isCheckUrl(url)) fail(LOOPBACK_URL_ERROR)", "  if (url === undefined) return\n  if (!isHttpOrHttpsUrl(url)) fail(\"url must be an http or https URL\")"),
             ("connect.ts", "  if (!isCheckUrl(url)) return { ok: false, message: LOOPBACK_URL_ERROR }\n", ""),
             ("job-cli.ts", "  if (url !== undefined && !isCheckUrl(url)) return { ok: false, message: LOOPBACK_URL_ERROR }\n", "")],
}
for mid, edits in M.items():
    for e in edits:
        f, old, new = e[0], e[1], e[2]
        p = os.path.join(SRC, f); s = open(p).read()
        n = s.count(old)
        if n == 0: sys.exit(f"{mid}: anchor not found in {f}: {old!r}")
        if n > 1 and (len(e) < 4): sys.exit(f"{mid}: anchor not unique in {f} ({n})")
        s = s.replace(old, new); open(p, "w").write(s)
    diff = subprocess.run(["git", "-C", WT, "diff", "-R", "--", "examples/auspex-ts/src"], capture_output=True, text=True).stdout
    open(os.path.join(OUT, f"{mid}.patch"), "w").write(diff)
    subprocess.run(["git", "-C", WT, "checkout", "--", "."], check=True)
    print(mid, "ok", diff.count("\n-") , "lines")
