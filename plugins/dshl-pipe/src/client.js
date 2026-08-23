// @dshl/pipe — control pipe client (legacy remote launcher backend).
//
// A tiny newline-delimited JSON client for the dshl control pipe. The
// endpoint URL format is `dshl://<token>@<host>:<port>`.
//
// One socket at a time; a dropped connection rejects in-flight requests and
// the next `request()` reconnects (the server authenticates each new socket
// with the hello handshake).
import { connect } from 'node:net'

function parseEndpoint(url) {
  if (typeof url !== 'string' || url.length === 0) throw new Error('missing endpoint')
  const rest = url.startsWith('dshl://') ? url.slice('dshl://'.length) : url
  const at = rest.lastIndexOf('@')
  const colon = rest.lastIndexOf(':')
  // Reason-only messages: the raw URL embeds the control bearer token, and
  // this class is exported — a caller that logs the error would leak it to
  // disk. Shape (`dshl://<token>@host:port`) is documented, so the cause is
  // recoverable without echoing the secret.
  if (at === -1 || colon <= at) throw new Error('invalid endpoint: expected dshl://<token>@host:port')
  const token = rest.slice(0, at)
  const host = rest.slice(at + 1, colon)
  const port = Number(rest.slice(colon + 1))
  if (!Number.isInteger(port) || port <= 0 || port > 65535) throw new Error('invalid endpoint: port out of range')
  return { token, host, port }
}

const REQUEST_TIMEOUT_MS = 15_000
const CONNECT_TIMEOUT_MS = 5_000

export class ControlClient {
  #token
  #host
  #port
  #socket = null
  #nextId = 1
  #pending = new Map()
  #buffer = ''
  #logger

  constructor(url, logger) {
    this.#logger = logger
    const { token, host, port } = parseEndpoint(url)
    this.#token = token
    this.#host = host
    this.#port = port
  }

  get connected() {
    return this.#socket !== null
  }

  async request(method, params = {}) {
    const socket = await this.#ensure()
    const id = this.#nextId++
    socket.write(JSON.stringify({ type: 'request', id, method, params }) + '\n')
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id)
        reject(new Error(`dshl control request timed out: ${method}`))
      }, REQUEST_TIMEOUT_MS)
      this.#pending.set(id, { resolve, reject, timer })
    })
  }

  #ensure() {
    if (this.#socket !== null) return Promise.resolve(this.#socket)
    return new Promise((resolve, reject) => {
      const socket = connect({ host: this.#host, port: this.#port })
      this.#socket = socket
      socket.setNoDelay(true)
      let settled = false
      const fail = (err) => {
        if (settled) return
        settled = true
        this.#close()
        reject(err)
      }
      socket.on('connect', () => {
        socket.write(JSON.stringify({ type: 'hello', token: this.#token }) + '\n')
        if (settled) return
        settled = true
        resolve(socket)
      })
      socket.on('data', (chunk) => this.#onData(chunk))
      socket.on('error', (err) => fail(err))
      socket.on('close', () => {
        const cause = new Error('dshl control connection closed')
        this.#close()
        this.#failPending(cause)
      })
      const timer = setTimeout(() => fail(new Error('dshl control connect timed out')), CONNECT_TIMEOUT_MS)
      socket.once('close', () => clearTimeout(timer))
    })
  }

  #onData(chunk) {
    this.#buffer += chunk.toString('utf8')
    let nl
    while ((nl = this.#buffer.indexOf('\n')) !== -1) {
      const line = this.#buffer.slice(0, nl).trim()
      this.#buffer = this.#buffer.slice(nl + 1)
      if (!line) continue
      let frame
      try {
        frame = JSON.parse(line)
      } catch {
        continue
      }
      this.#handleResponse(frame)
    }
  }

  #handleResponse(frame) {
    if (frame === null || typeof frame !== 'object') return
    const id = frame.id
    if (typeof id !== 'number') return
    const entry = this.#pending.get(id)
    if (entry === undefined) return
    this.#pending.delete(id)
    clearTimeout(entry.timer)
    if (frame.error !== undefined) {
      entry.reject(new Error(String(frame.error)))
    } else {
      entry.resolve(frame.result)
    }
  }

  #failPending(cause) {
    for (const [, entry] of this.#pending) {
      clearTimeout(entry.timer)
      entry.reject(cause)
    }
    this.#pending.clear()
  }

  #close() {
    if (this.#socket !== null) {
      this.#socket.destroy()
      this.#socket = null
    }
  }

  dispose() {
    this.#failPending(new Error('dshl control client disposed'))
    this.#close()
  }
}
