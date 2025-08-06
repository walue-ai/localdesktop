# Local Desktop

Local Desktop helps you run a desktop Linux environment on your Android device.

> Local Desktop is a Rust rewrite of the original [Polar Bear](https://github.com/localdesktop/polar-bear) project, which was written in Kotlin and C++. The aim of this rewrite is to make it more stable, portable, and able to do the development work on Android.

## How it works

1. An Arch Linux ARM64 filesystem is set up inside the app's internal storage.
2. Proot mounts the filesystem and provides a chroot-like environment.
3. A minimal built-in Wayland compositor runs in Android NDK.
4. Rootful Xwayland & a desktop environment launches inside the chroot and renders back to the Android native activity.

![POC](./assets/docs/proof-of-concept.png)
_Proof of Concept: A Pixel Tablet running the XFCE desktop environment inside a Proot-based ARM64 Linux. No configuration is required; just install and go._

## Getting Started

### How to build an APK

#### Prerequisites

Before building Android APKs, ensure you have the following dependencies installed:

1. **Android NDK and SDK**: 
   ```bash
   # Install Android command line tools
   sudo mkdir -p /opt/android-sdk
   cd /opt/android-sdk
   sudo wget https://dl.google.com/android/repository/commandlinetools-linux-11076708_latest.zip
   sudo unzip commandlinetools-linux-11076708_latest.zip
   sudo mkdir -p cmdline-tools/latest
   sudo mv cmdline-tools/* cmdline-tools/latest/ 2>/dev/null || true
   sudo chown -R $USER:$USER /opt/android-sdk
   
   # Set environment variables
   export ANDROID_HOME=/opt/android-sdk
   export PATH=$PATH:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools
   
   # Accept licenses and install required components
   yes | sdkmanager --licenses
   sdkmanager "platform-tools" "build-tools;34.0.0" "platforms;android-34"
   ```

2. **Build tools**:
   ```bash
   # Install required system packages
   sudo apt update
   sudo apt install -y openjdk-17-jdk clang lld wget unzip
   
   # Install xbuild
   cargo install xbuild
   ```

#### Building APK

```bash
# Build debug APK
x build --platform android --arch arm64

# Build release APK  
x build --platform android --arch arm64 --release

# Build Android App Bundle (AAB)
x build --platform android --arch arm64 --format aab
```

The generated APK files will be located in:
- Debug: `target/x/debug/android/gradle/app/build/outputs/apk/debug/app-debug.apk`
- Release: `target/x/debug/android/gradle/app/build/outputs/apk/release/app-release-unsigned.apk`

AAB files will be in:
- Debug: `target/x/debug/android/gradle/app/build/outputs/bundle/debug/app-debug.aab`
- Release: `target/x/debug/android/gradle/app/build/outputs/bundle/release/app-release.aab`

Alternatively, trigger the default build task by pressing `Ctrl+Shift+B`.

### How to develop

Recommended setup:

- **IDE**: Visual Studio Code

- **Extensions**:

  - [Rust Analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
  - [Android Debug](https://marketplace.visualstudio.com/items?itemName=nisargjhaveri.android-debug)
  - [CodeLLDB](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb)

- **Instructions**:

  1. Open Visual Studio Code.
  2. Launch the `[Android] Debug` configuration from the debug panel. This will:
     - Build the Rust code into an APK.
     - Install the APK on a selectable device, and launch the app in debug mode.
     - Redirect stdout/stderr output to the terminal.

![It is easy to debug](./assets/docs/debugable.webp)

> **Tip**: You can debug the app on either a physical device or a virtual device.

For more instructions on how to work on this project, please visit the [Developer Manual](https://localdesktop.github.io/docs/developer/how-it-works).
