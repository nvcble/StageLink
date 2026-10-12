const path = require('node:path')

const repoRoot = path.resolve(__dirname, '..', '..')

/**
 * electron-builder configuration for StageLink Core.
 *
 * Three things are bundled into one installer:
 *
 * 1. The Electron main and preload bundles (`dist-electron`).
 * 2. The generated Nuxt renderer (`.output/public`), served in production over
 *    the `stagelink://` protocol rather than `file://`.
 * 3. The compiled Rust audio service, as an extra resource so it can be spawned
 *    as a normal child process.
 *
 * The renderer and the service go in `extraResources` rather than inside the
 * asar: the service is an executable (which cannot be run from an archive), and
 * the renderer is served by a protocol handler that reads real files.
 *
 * @type {import('electron-builder').Configuration}
 */
module.exports = {
  // electron-builder needs an exact version: the workspace pins a caret range
  // so npm can take patch updates, and resolving it here keeps the two from
  // disagreeing about which Electron binaries to download.
  electronVersion: require('electron/package.json').version,

  appId: 'org.stagelink.core',
  productName: 'StageLink Core',
  copyright: `Copyright © ${new Date().getFullYear()} StageLink`,

  directories: {
    output: 'release',
    buildResources: path.join(repoRoot, 'assets', 'branding', 'generated'),
  },

  // The app package holds the Electron bundles plus the production
  // dependencies they require at runtime; the renderer and the Rust service are
  // extra resources.
  //
  // Note what is deliberately NOT here: an exclusion for node_modules. tsup
  // externalises anything listed in `dependencies`, so electron-log is required
  // at runtime rather than bundled. Excluding node_modules therefore produced a
  // package that died on its very first require — before logging was
  // initialised, which made the failure completely silent. electron-builder's
  // default production-dependency walk is the correct mechanism.
  files: [
    'dist-electron/**/*',
    'package.json',
    '!**/*.map',
  ],

  extraResources: [
    {
      // Served over stagelink:// by electron/main/protocol.ts, which looks in
      // `process.resourcesPath/renderer`.
      from: '.output/public',
      to: 'renderer',
      filter: ['**/*'],
    },
    {
      // Spawned by the service supervisor, which looks in
      // `process.resourcesPath/service`.
      from: path.join(repoRoot, 'crates', 'target', 'release'),
      to: 'service',
      filter: ['stagelink-service.exe'],
    },
    {
      // The tray icon is read from `process.resourcesPath/branding`.
      from: path.join(repoRoot, 'assets', 'branding', 'generated'),
      to: 'branding',
      filter: ['tray.png'],
    },
  ],

  asar: true,
  // The Rust service must stay a real file on disk to be executable.
  asarUnpack: [],

  win: {
    target: [
      { target: 'nsis', arch: ['x64'] },
    ],
    icon: path.join(repoRoot, 'assets', 'branding', 'generated', 'icon.ico'),
    // Unsigned builds are expected during development. Set
    // CSC_LINK / CSC_KEY_PASSWORD in CI to sign a release; an unsigned
    // installer triggers a SmartScreen warning that an operator installing
    // before a service should not have to reason about.
    signAndEditExecutable: true,
    requestedExecutionLevel: 'asInvoker',
  },

  nsis: {
    oneClick: false,
    perMachine: false,
    allowToChangeInstallationDirectory: true,
    createDesktopShortcut: true,
    createStartMenuShortcut: true,
    shortcutName: 'StageLink Core',
    installerIcon: path.join(repoRoot, 'assets', 'branding', 'generated', 'icon.ico'),
    uninstallerIcon: path.join(repoRoot, 'assets', 'branding', 'generated', 'icon.ico'),
    // The database, logs and secrets live in %LOCALAPPDATA%\StageLink and are
    // deliberately *not* removed on uninstall: an operator reinstalling before
    // a service must not lose their device selection and paired receivers.
    // `include` would be the place to offer that as an explicit choice.
    deleteAppDataOnUninstall: false,
    artifactName: 'StageLink-Core-Setup-${version}-${arch}.${ext}',
  },

  // No auto-update: StageLink is designed to run on an isolated network with no
  // internet access, so an updater would be both useless and a source of
  // surprise mid-service.
  publish: null,
}
