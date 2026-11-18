'use strict'

// A desktop window for the npack web store. The store itself is served by the
// npack daemon (`npack daemon --web`) from a random, authenticated loopback
// port; this app only starts that daemon, shows its page and stops it again.

const { app, BrowserWindow, dialog, shell, session } = require('electron')
const { spawn } = require('node:child_process')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const STARTUP_TIMEOUT_MS = 20000
const URL_LINE = /npack web store: (http:\/\/127\.0\.0\.1:\d+\/#token=[0-9a-f]+)/

let daemon = null
let quitting = false

/** Where to find the npack executable, most specific first. */
function npackCandidates() {
  const exe = path.dirname(app.getPath('exe'))
  return [
    process.env.NPACK_BIN,
    // A user-local .npk install: <prefix>/share/npack-gui/npack-gui next to <prefix>/bin/npack.
    path.resolve(exe, '..', '..', 'bin', 'npack'),
    '/usr/bin/npack',
    '/usr/local/bin/npack',
    path.join(os.homedir(), '.local', 'bin', 'npack'),
  ].filter(Boolean)
}

function findNpack() {
  for (const candidate of npackCandidates()) {
    try {
      fs.accessSync(candidate, fs.constants.X_OK)
      return candidate
    } catch {
      // try the next one
    }
  }
  return 'npack' // fall back to PATH
}

/** Starts `npack daemon --web` on a free port and resolves with its launch URL. */
function startDaemon() {
  return new Promise((resolve, reject) => {
    const runtime = process.env.XDG_RUNTIME_DIR || os.tmpdir()
    // Its own socket, so it never fights a web-enabled npackd started elsewhere.
    const socket = path.join(runtime, `npackd-gui-${process.pid}.sock`)
    const binary = findNpack()
    let output = ''
    let settled = false
    const finish = (error, url) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      error ? reject(error) : resolve(url)
    }

    daemon = spawn(binary, ['daemon', '--web', '--web-port', '0', '--socket', socket], {
      stdio: ['ignore', 'ignore', 'pipe'],
    })
    const timer = setTimeout(
      () => finish(new Error(`npack did not start within ${STARTUP_TIMEOUT_MS / 1000} seconds.\n\n${output}`)),
      STARTUP_TIMEOUT_MS
    )
    daemon.stderr.setEncoding('utf8')
    daemon.stderr.on('data', (chunk) => {
      output = (output + chunk).slice(-4000)
      const match = URL_LINE.exec(output)
      if (match) finish(null, match[1])
    })
    daemon.on('error', (error) =>
      finish(new Error(`Could not run "${binary}": ${error.message}\n\nInstall the npack package, or set NPACK_BIN.`))
    )
    daemon.on('exit', (code, signal) => {
      finish(new Error(`npack exited early (${signal || code}).\n\n${output}`))
      if (!quitting) {
        dialog.showErrorBox('npack stopped', `The npack service exited unexpectedly (${signal || code}).`)
        app.quit()
      }
      try {
        fs.unlinkSync(socket)
      } catch {
        // already gone
      }
    })
  })
}

function stopDaemon() {
  quitting = true
  if (daemon && daemon.exitCode === null) daemon.kill('SIGTERM')
}

function createWindow(launchUrl) {
  const origin = new URL(launchUrl).origin
  const window = new BrowserWindow({
    width: 1100,
    height: 760,
    minWidth: 640,
    minHeight: 480,
    title: 'npack',
    backgroundColor: '#14161a',
    autoHideMenuBar: true,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
    },
  })
  window.removeMenu()

  // Only the store's own page may load here; everything else opens in the
  // user's browser (web links only) and never inside this window.
  const external = (url) => {
    try {
      const parsed = new URL(url)
      if (parsed.protocol === 'https:' || parsed.protocol === 'http:') shell.openExternal(url)
    } catch {
      // ignore malformed URLs
    }
  }
  window.webContents.setWindowOpenHandler(({ url }) => {
    external(url)
    return { action: 'deny' }
  })
  window.webContents.on('will-navigate', (event, url) => {
    if (new URL(url).origin !== origin) {
      event.preventDefault()
      external(url)
    }
  })
  window.loadURL(launchUrl)
  return window
}

const gotLock = app.requestSingleInstanceLock()
if (!gotLock) {
  app.quit()
} else {
  app.setDesktopName('io.npack.Gui.desktop')
  let mainWindow = null
  app.on('second-instance', () => {
    if (mainWindow) {
      if (mainWindow.isMinimized()) mainWindow.restore()
      mainWindow.focus()
    }
  })
  app.on('before-quit', stopDaemon)
  app.on('window-all-closed', () => app.quit())
  process.on('exit', stopDaemon)
  for (const signal of ['SIGTERM', 'SIGINT', 'SIGHUP']) process.on(signal, () => app.quit())

  app.whenReady().then(async () => {
    // The store needs no device or notification access.
    session.defaultSession.setPermissionRequestHandler((_wc, _permission, callback) => callback(false))
    try {
      const url = await startDaemon()
      mainWindow = createWindow(url)
    } catch (error) {
      dialog.showErrorBox('npack could not start', String(error.message || error))
      app.quit()
    }
  })
}
