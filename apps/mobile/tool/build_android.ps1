# Cross-compiles the Rust receiver for Android and puts it where Gradle looks.
#
# Run this before `flutter build apk` or `flutter run`. Gradle does not know
# how to drive Cargo, and teaching it would couple two build systems for no
# gain — so this is a separate, visible step.
#
#   pwsh apps/mobile/tool/build_android.ps1
#
# Prerequisites, once:
#   rustup target add aarch64-linux-android x86_64-linux-android
#   The Android NDK (any recent version); the path is in crates/.cargo/config.toml

$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$app = Split-Path -Parent $here
$repo = Split-Path -Parent (Split-Path -Parent $app)
$crates = Join-Path $repo 'crates'
$jniLibs = Join-Path $app 'android/app/src/main/jniLibs'

# ABI name as Android calls it, and the Rust target triple it comes from.
$targets = @(
  @{ Abi = 'arm64-v8a'; Triple = 'aarch64-linux-android' },
  @{ Abi = 'x86_64';    Triple = 'x86_64-linux-android' }
)

# Cargo is often not on PATH in a fresh shell, and "cargo is not recognized" is
# a worse error than one that says where it looked.
$cargo = 'cargo'
if (-not (Get-Command $cargo -ErrorAction SilentlyContinue)) {
  $fallback = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
  if (Test-Path $fallback) {
    $cargo = $fallback
  } else {
    throw "cargo was not found on PATH or at $fallback. Install Rust from https://rustup.rs"
  }
}

Write-Host 'Building the StageLink receiver for Android' -ForegroundColor Cyan

foreach ($target in $targets) {
  $triple = $target.Triple
  $abi = $target.Abi

  Write-Host "  $abi ($triple)" -ForegroundColor Gray

  # `--profile mobile`, never `--release`.
  #
  # The workspace release profile sets `panic = "abort"`, which is right for a
  # standalone service and wrong here: receiver-ffi catches panics at every
  # `extern "C"` entry point so one cannot unwind into Dart or Kotlin, and
  # `catch_unwind` does nothing under `abort`. The `mobile` profile is release
  # with `panic = "unwind"`.
  Push-Location $crates
  try {
    & $cargo build -p receiver-ffi --profile mobile --target $triple
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed for $triple" }
  } finally {
    Pop-Location
  }

  $source = Join-Path $crates "target/$triple/mobile/libstagelink_receiver.so"
  if (-not (Test-Path $source)) { throw "expected $source to exist" }

  $destination = Join-Path $jniLibs $abi
  New-Item -ItemType Directory -Force -Path $destination | Out-Null
  Copy-Item $source $destination -Force

  $size = [math]::Round((Get-Item $source).Length / 1MB, 1)
  Write-Host "    -> $destination  ($size MB)" -ForegroundColor DarkGray
}

Write-Host 'Done. Now run: flutter build apk' -ForegroundColor Green
