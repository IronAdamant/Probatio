# Holdout mutants put back by hand on today's src (b38e85f). Not inspected against any suite while
# the improved suite is designed.
import subprocess, sys, os
WT = sys.argv[1]; OUT = sys.argv[2]
SRC = os.path.join(WT, "examples/auspex-ts/src")
M = {
 "ba32203": [("connect.ts", "Receipt: ${tildePath(jobFilePath(job.jobId))}", "Receipt: ${jobFilePath(job.jobId)}")],
 "4404714": [("text.ts", "  let inner = excerptOf(maskSecrets(opts.raw))", "  let inner = excerptOf(opts.raw)")],
 "c1cf241": [("solari.ts", 'import { hydrateSessionStorage, installSessionStorageRestore } from "./profile-storage.ts"', 'import { hydrateSessionStorage, installSessionStorageRestore, isPersistableAppUrl } from "./profile-storage.ts"'),
             ("solari.ts", "  if (opts.profile && restored > 0) {", "  if (opts.profile && restored > 0 && !isPersistableAppUrl(page.url())) {")],
 "472fdd1": [("job-store.ts", 'if (waitMs <= 0 || current.phase === "completed" || current.phase === "failed") return publicJob(current)', "if (waitMs <= 0) return publicJob(current)")],
 "2605f49": [("argv.ts", "  if (value === undefined || looksLikeFlag(value)) return undefined", "  if (!value || looksLikeFlag(value)) return undefined")],
 "135692f": [("check-reason.ts", "    if (opts.verify.anonymousClaimSkipped) return opts.verify.ok\n", "")],
 "525de86": [("handoff-doors.ts", "  for (let i = 0; i < limit; i++) {", "  for (let i = 1; i < limit; i++) {")],
 "543c7f3": [("fold-steer.ts", '  if (live && site && hostIs(live, site)) return "app-visible"', '  if (false && live && site && hostIs(live, site)) return "app-visible"')],
 "7207d9e": [("fold-steer.ts", '  return idpOnlyKind({ liveHost: opts.liveHost, siteHost: opts.siteHost }) === "app-visible"', "  return false")],
}
for mid, edits in M.items():
    for f, old, new in edits:
        p = os.path.join(SRC, f); s = open(p).read()
        if s.count(old) != 1: sys.exit(f"{mid}: anchor count {s.count(old)} in {f}")
        open(p, "w").write(s.replace(old, new))
    diff = subprocess.run(["git", "-C", WT, "diff", "-R", "--", "examples/auspex-ts/src"], capture_output=True, text=True).stdout
    open(os.path.join(OUT, f"h-{mid}.patch"), "w").write(diff)
    subprocess.run(["git", "-C", WT, "checkout", "--", "."], check=True)
    print(mid, "ok")
