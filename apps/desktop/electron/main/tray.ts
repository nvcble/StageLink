import path from 'node:path'
import { existsSync } from 'node:fs'
import { BrowserWindow, Menu, Tray, app, nativeImage } from 'electron'

import { logger } from './logger'
import { isPackaged } from './paths'

/**
 * System tray presence.
 *
 * A live monitor system is often left running on a laptop pushed to one side of
 * the stage, so the window gets minimised but the service must keep going. The
 * tray is how the operator gets back to it, and — importantly — the only
 * obvious way to actually quit.
 */

let tray: Tray | null = null

/** Locates the tray icon generated from the official StageLink logo. */
function trayIconPath(): string | null {
  const candidates = isPackaged()
    ? [path.join(process.resourcesPath, 'branding', 'tray.png')]
    : [
        path.resolve(app.getAppPath(), '..', '..', 'assets', 'branding', 'generated', 'tray.png'),
        path.resolve(app.getAppPath(), '..', '..', 'assets', 'branding', 'StageLink-Logo.png'),
      ]
  return candidates.find((candidate) => existsSync(candidate)) ?? null
}

export function createTray(
  getWindow: () => BrowserWindow | null,
  onQuit: () => void,
): Tray | null {
  const iconPath = trayIconPath()
  if (!iconPath) {
    // Not fatal: the application works without a tray icon. Say so rather than
    // substituting a different image.
    logger.warn('no StageLink tray icon found; run `npm run icons` to generate one')
    return null
  }

  const image = nativeImage.createFromPath(iconPath).resize({ width: 16, height: 16 })
  tray = new Tray(image)
  tray.setToolTip('StageLink Core')

  const show = () => {
    const window = getWindow()
    if (!window) return
    if (window.isMinimized()) window.restore()
    window.show()
    window.focus()
  }

  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: 'StageLink Core', enabled: false },
      { type: 'separator' },
      { label: 'Open StageLink', click: show },
      { type: 'separator' },
      { label: 'Quit StageLink', click: onQuit },
    ]),
  )

  tray.on('double-click', show)
  return tray
}

export function destroyTray(): void {
  tray?.destroy()
  tray = null
}
