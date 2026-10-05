// node --test custom reporter. One JSON object on stdout: failures with the file
// the runner itself recorded. TAP headings are not used.
export default async function* reporter(source) {
  const failed = []
  const names = []
  let tests = 0
  let pass = 0
  let fail = 0
  for await (const event of source) {
    if (event.type === "test:pass" || event.type === "test:fail") {
      const data = event.data ?? {}
      if (data.details && data.details.type === "suite") continue
      const name = String(data.name ?? "")
      const file = String(data.file ?? "")
      names.push(name)
      if (event.type === "test:fail") {
        const failure = { name, file, line: Number(data.line ?? 0) }
        // An open handle is reported as a timeout whose name is the file path.
        // failureType lives on the error; the text is on error.cause, not message.
        if (fileTimedOut(name, file, data.details?.error)) failure.fileTimeout = true
        failed.push(failure)
      }
    } else if (event.type === "test:diagnostic") {
      const message = String(event.data?.message ?? "")
      const match = message.match(/^(tests|pass|fail) (\d+)$/)
      if (!match) continue
      const count = Number(match[2])
      if (match[1] === "tests") tests = count
      else if (match[1] === "pass") pass = count
      else fail = count
    }
  }
  yield JSON.stringify({ tests, pass, fail, failed, names })
}

function fileTimedOut(name, file, error) {
  if (!error || typeof error !== "object") return false
  const cause = typeof error.cause === "string" ? error.cause : ""
  const text = `${error.failureType ?? ""} ${error.message ?? ""} ${cause}`
  if (!/testTimeoutFailure|timed out/i.test(text)) return false
  const base = String(file).split(/[/\\]/).pop()
  if (!base) return false
  return name === file || name === base || name.endsWith(`/${base}`) || name.endsWith(`\\${base}`)
}
