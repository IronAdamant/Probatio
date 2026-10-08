// Loaded by the wrapper `probatio golden record` puts in front of a module.
// Each call to an exported function is written to PROBATIO_GOLDEN_LOG: the arguments as they were
// before the call, and what it returned or threw. A call whose arguments or result are not plain
// data (a callback, a class instance, a stubbed global) is a hidden input. It is counted, not recorded.
import { appendFileSync } from "node:fs"

const LOG = process.env.PROBATIO_GOLDEN_LOG

/** `{ u: true }` for undefined, `{ url }` for a URL, `{ v }` for plain JSON data, null otherwise. */
export function encode(value) {
  if (value === undefined) return { u: true }
  if (value instanceof URL) return { url: value.href }
  return plain(value, 0) ? { v: JSON.parse(JSON.stringify(value)) } : null
}

function plain(value, depth) {
  if (depth > 40) return false
  if (value === null || typeof value === "string" || typeof value === "boolean") return true
  if (typeof value === "number") return Number.isFinite(value) && !Object.is(value, -0)
  if (Array.isArray(value)) return value.every((item) => item !== undefined && plain(item, depth + 1))
  if (typeof value === "object") {
    const proto = Object.getPrototypeOf(value)
    if (proto !== Object.prototype && proto !== null) return false
    return Object.values(value).every((item) => item !== undefined && plain(item, depth + 1))
  }
  return false
}

function errorOf(error) {
  return { name: typeof error?.name === "string" ? error.name : "Error", message: String(error?.message ?? error) }
}

function write(row) {
  if (!LOG) return
  try {
    appendFileSync(LOG, `${JSON.stringify(row)}\n`)
  } catch {
    // A full disk must not fail the test that is being recorded.
  }
}

export function record(module, name, fn) {
  if (typeof fn !== "function") return fn
  if (/^class[\s{]/.test(Function.prototype.toString.call(fn))) return fn
  const wrapped = function (...args) {
    const encoded = args.map(encode)
    const base = { module, fn: name, at: Date.now() }
    if (encoded.some((item) => item === null)) {
      write({ ...base, hidden: "argument" })
      return fn.apply(this, args)
    }
    let out
    try {
      out = fn.apply(this, args)
    } catch (error) {
      write({ ...base, args: encoded, error: errorOf(error) })
      throw error
    }
    if (out && typeof out.then === "function") {
      return out.then(
        (value) => {
          const result = encode(value)
          write(result ? { ...base, args: encoded, async: true, result } : { ...base, hidden: "result" })
          return value
        },
        (error) => {
          write({ ...base, args: encoded, async: true, error: errorOf(error) })
          throw error
        },
      )
    }
    const result = encode(out)
    write(result ? { ...base, args: encoded, result } : { ...base, hidden: "result" })
    return out
  }
  Object.defineProperty(wrapped, "name", { value: fn.name })
  return wrapped
}
