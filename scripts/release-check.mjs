#!/usr/bin/env node
// Runs before `npm publish` (prepublishOnly). A version reaches npm only after the commit it names
// has a green test run, a green self-score run, a CHANGELOG entry, and a pushed v<version> tag.
// The order and the reasons are in docs/releasing.md. `npm publish --ignore-scripts` skips this on
// purpose; say why in the CHANGELOG entry when that happens.
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

/** Each check returns a line. `run(bin, args)` is injectable so the checks can be tested without a network. */
export function releaseChecks({ run, version, changelog, name }) {
  const out = (bin, args) => {
    const result = run(bin, args)
    return { ok: result.status === 0, text: String(result.stdout ?? "").trim() }
  }
  const checks = []
  const add = (ok, label, fix) => checks.push({ ok, label, fix: ok ? "" : fix })
  const status = out("git", ["status", "--porcelain"])
  add(status.ok && status.text === "", "the working tree is clean", "Commit or stash every change first.")
  const branch = out("git", ["rev-parse", "--abbrev-ref", "HEAD"])
  add(branch.text === "main", "the release is cut from main", "Switch to main.")
  const head = out("git", ["rev-parse", "HEAD"]).text
  const remote = out("git", ["ls-remote", "origin", "refs/heads/main"]).text.split(/\s+/)[0] ?? ""
  add(head !== "" && head === remote, "HEAD is the pushed main", "Push main first: git push origin main.")
  const published = out("npm", ["view", `${name}@${version}`, "version"])
  add(published.text === "", `${version} is not on npm yet`, `${version} is already published. Bump the version.`)
  add(new RegExp(`^## ${version.replace(/\./g, "\\.")}\\s*$`, "m").test(changelog), `CHANGELOG.md has a ${version} entry`, `Add "## ${version}" to CHANGELOG.md.`)
  const tag = `v${version}`
  const local = out("git", ["rev-parse", `${tag}^{commit}`])
  const pushed = out("git", ["ls-remote", "--tags", "origin", `refs/tags/${tag}^{}`, `refs/tags/${tag}`]).text
  const pushedSha = pushed.split("\n").map((line) => line.split(/\s+/)).find(([, ref]) => ref?.endsWith("^{}"))?.[0] ?? pushed.split(/\s+/)[0] ?? ""
  add(local.ok && local.text === head && pushedSha === head, `${tag} is pushed and points at HEAD`, `git tag -a ${tag} -m ${tag} && git push origin ${tag}`)
  for (const workflow of ["test.yml", "self-score.yml"]) {
    const listed = out("gh", ["run", "list", "--commit", head, "--workflow", workflow, "--json", "status,conclusion", "--limit", "1"])
    let green = false
    try {
      const [latest] = JSON.parse(listed.text || "[]")
      green = latest?.status === "completed" && latest?.conclusion === "success"
    } catch {
      green = false
    }
    add(green, `${workflow} is green on ${head.slice(0, 7)}`, `Wait for ${workflow} on this commit, or fix it. gh run list --commit ${head} --workflow ${workflow}`)
  }
  return { ok: checks.every((check) => check.ok), checks }
}

const here = path.dirname(fileURLToPath(import.meta.url))
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(here, "..")
  const pkg = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8"))
  const result = releaseChecks({
    name: pkg.name,
    version: pkg.version,
    changelog: readFileSync(path.join(root, "CHANGELOG.md"), "utf8"),
    run: (bin, args) => spawnSync(bin, args, { cwd: root, encoding: "utf8" }),
  })
  for (const check of result.checks) process.stdout.write(`${check.ok ? "ok  " : "not "} ${check.label}${check.fix ? `\n     ${check.fix}` : ""}\n`)
  if (!result.ok) {
    process.stdout.write("Not publishing. See docs/releasing.md.\n")
    process.exit(1)
  }
  process.stdout.write(`Release checks passed for ${pkg.name}@${pkg.version}.\n`)
}
