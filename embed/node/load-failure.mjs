// A test file that failed before any test ran either did not link or threw while loading.
// Did not link: a removed export, a missing module, a file that does not parse. No code ran.
// A compiled language rejects the same change at compile time, so that mutant is unviable.
// Threw while loading: the file's own top-level code ran the program and it broke. That is a kill.

const STATIC_ERROR = /^(?:SyntaxError|(?:Type)?Error \[ERR_(?:MODULE_NOT_FOUND|UNKNOWN_FILE_EXTENSION|UNSUPPORTED_DIR_IMPORT|PACKAGE_PATH_NOT_EXPORTED)\]):/

/**
 * `text` is the error as printed: a `Name: message` line, then `at` frames.
 * Static when the error is a link or parse error and no frame is in the project's own code.
 * A SyntaxError thrown by running code (JSON.parse at the top of a test) has a frame in that file.
 */
export function staticLoadFailure(text) {
  const lines = String(text).split(/\r?\n/)
  const head = lines.findIndex((line) => STATIC_ERROR.test(line.trim()))
  if (head === -1) return false
  for (const raw of lines.slice(head + 1)) {
    const line = raw.trim()
    if (!line.startsWith("at ")) break
    if (projectFrame(line)) return false
  }
  return true
}

/** The error object a node:test event carries, printed the way node prints it. */
export function errorText(error) {
  if (!error || typeof error !== "object") return ""
  const name = typeof error.name === "string" ? error.name : "Error"
  const code = typeof error.code === "string" ? ` [${error.code}]` : ""
  const message = typeof error.message === "string" ? error.message : ""
  const stack = typeof error.stack === "string" ? error.stack.split("\n").slice(1).join("\n") : ""
  return `${name}${code}: ${message}\n${stack}`
}

function projectFrame(frame) {
  if (/\bnode:/.test(frame) || frame.includes("<anonymous>")) return false
  const location = /(?:file:\/\/)?(\/[^\s)]+|[A-Za-z]:\\[^\s)]+):\d+:\d+\)?$/.exec(frame)
  if (!location) return false
  return !location[1].split(/[\\/]/).includes("node_modules")
}
