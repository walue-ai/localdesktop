use crate::{task, BuildEnv, Format, Opt};
use anyhow::{Context, Result};
use apk::Target;
use std::path::{Path, PathBuf};
use std::process::Command;

static BUILD_GRADLE: &[u8] = include_bytes!("./build.gradle");
static GRADLE_PROPERTIES: &[u8] = include_bytes!("./gradle.properties");
static SETTINGS_GRADLE: &[u8] = include_bytes!("./settings.gradle");
static IC_LAUNCHER: &[u8] = include_bytes!("./ic_launcher.xml");

pub fn prepare(env: &BuildEnv) -> Result<()> {
    let config = env.config().android();
    if config.wry {
        let package = config.manifest.package.as_ref().unwrap();
        let wry = env.platform_dir().join("wry");
        std::fs::create_dir_all(&wry)?;
        if !env.cargo().package_root().join("kotlin").exists() {
            let main_activity = format!(
                r#"
                    package {}
                    class MainActivity : TauriActivity()
                "#,
                package,
            );
            std::fs::write(wry.join("MainActivity.kt"), main_activity)?;
        }
        let (package, name) = package.rsplit_once('.').unwrap();
        std::env::set_var("WRY_ANDROID_REVERSED_DOMAIN", package);
        std::env::set_var("WRY_ANDROID_APP_NAME_SNAKE_CASE", name);
        std::env::set_var("WRY_ANDROID_KOTLIN_FILES_OUT_DIR", wry);
    }
    Ok(())
}

pub fn build(env: &BuildEnv, libraries: Vec<(Target, PathBuf)>, out: &Path) -> Result<()> {
    let platform_dir = env.platform_dir();
    let gradle = platform_dir.join("gradle");
    let app = gradle.join("app");
    let main = app.join("src").join("main");
    let kotlin = main.join("kotlin");
    let jnilibs = main.join("jniLibs");
    let res = main.join("res");
    let assets = main.join("assets");

    std::fs::create_dir_all(&kotlin)?;
    std::fs::create_dir_all(&assets)?;
    std::fs::write(gradle.join("build.gradle"), BUILD_GRADLE)?;
    std::fs::write(gradle.join("gradle.properties"), GRADLE_PROPERTIES)?;
    std::fs::write(gradle.join("settings.gradle"), SETTINGS_GRADLE)?;

    let config = env.config().android();
    let mut manifest = config.manifest.clone();

    let package = manifest.package.take().unwrap_or_default();
    let target_sdk = manifest.sdk.target_sdk_version.take().unwrap();
    let min_sdk = manifest.sdk.min_sdk_version.take().unwrap();
    let version_code = manifest.version_code.take().unwrap();
    let version_name = manifest.version_name.take().unwrap();

    manifest.compile_sdk_version = None;
    manifest.compile_sdk_version_codename = None;
    manifest.platform_build_version_code = None;
    manifest.platform_build_version_name = None;
    manifest.application.debuggable = None;

    let mut dependencies = String::new();
    for dep in &config.dependencies {
        dependencies.push_str(&format!("implementation '{}'\n", dep));
    }

    let app_build_gradle = format!(
        r#"
            plugins {{
                id 'com.android.application'
                id 'org.jetbrains.kotlin.android'
            }}
            android {{
                namespace '{package}'
                compileSdk {target_sdk}
                defaultConfig {{
                    applicationId '{package}'
                    minSdk {min_sdk}
                    targetSdk {target_sdk}
                    versionCode {version_code}
                    versionName '{version_name}'
                }}
                packagingOptions {{
                    jniLibs {{
                        useLegacyPackaging true
                    }}
                }}
                def keystoreFile = file("release-key.jks")
                if (keystoreFile.exists()) {{
                    signingConfigs {{
                        release {{
                            storeFile keystoreFile
                            storePassword System.getenv("ANDROID_KEYSTORE_PASSWORD")
                            keyAlias System.getenv("ANDROID_KEY_ALIAS")
                            keyPassword System.getenv("ANDROID_KEY_PASSWORD")
                        }}
                    }}
                }}
                buildTypes {{
                    release {{
                        if (keystoreFile.exists()) {{
                            signingConfig signingConfigs.release
                        }}
                        minifyEnabled true
                        proguardFiles getDefaultProguardFile('proguard-android-optimize.txt'), 'proguard-rules.pro'
                    }}
                }}
            }}
            dependencies {{
                {dependencies}
            }}
        "#,
        package = package,
        target_sdk = target_sdk,
        min_sdk = min_sdk,
        version_code = version_code,
        version_name = version_name,
        dependencies = dependencies,
    );

    if let Some(icon_path) = env.icon.as_ref() {
        let mut scaler = xcommon::Scaler::open(icon_path)?;
        scaler.optimize();
        let anydpi = res.join("mipmap-anydpi-v26");
        std::fs::create_dir_all(&anydpi)?;
        std::fs::write(anydpi.join("ic_launcher.xml"), IC_LAUNCHER)?;
        let dpis = [
            ("m", 48),
            ("h", 72),
            ("xh", 96),
            ("xxh", 144),
            ("xxh", 192),
            ("xxxh", 256),
        ];
        for (name, size) in dpis {
            let dir_name = format!("mipmap-{}dpi", name);
            let dir = res.join(dir_name);
            std::fs::create_dir_all(&dir)?;
            for variant in ["foreground", "monochrome"] {
                let mut icon =
                    std::fs::File::create(dir.join(format!("ic_launcher_{}.png", variant)))?;
                scaler.write(
                    &mut icon,
                    xcommon::ScalerOptsBuilder::new(size, size).build(),
                )?;
            }
        }
        manifest.application.icon = Some("@mipmap/ic_launcher".into());
    }

    std::fs::write(app.join("build.gradle"), app_build_gradle)?;
    std::fs::write(
        main.join("AndroidManifest.xml"),
        quick_xml::se::to_string(&manifest)?,
    )?;

    let srcs = [
        env.cargo().package_root().join("kotlin"),
        env.platform_dir().join("wry"),
    ];
    for src in srcs {
        if !src.exists() {
            continue;
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            std::fs::copy(entry.path(), kotlin.join(entry.file_name()))?;
        }
    }

    for (target, lib) in libraries {
        let name = lib.file_name().context("invalid path")?;
        let lib_dir = jnilibs.join(target.as_str());
        std::fs::create_dir_all(&lib_dir)?;
        std::fs::copy(&lib, lib_dir.join(name))?;
    }

    // Handle assets
    for asset in &config.assets {
        let source_path = env.cargo().package_root().join(asset.path());
        if !asset.optional() || source_path.exists() {
            let file_name = asset
                .path()
                .file_name()
                .context("Asset must have file_name component")?;
            let dest_path = assets.join(file_name);
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source_path, &dest_path)?;
        }
    }

    let opt = env.target().opt();
    let format = env.target().format();
    create_gradle_wrapper(&gradle)?;
    
    let mut cmd = if gradle.join("gradlew").exists() {
        let gradlew_path = gradle.join("gradlew");
        Command::new(gradlew_path)
    } else {
        Command::new("gradle")
    };
    
    cmd.current_dir(&gradle);
    cmd.arg(match format {
        Format::Aab => "bundle",
        Format::Apk => "assemble",
        _ => unreachable!(),
    });
    println!("cmd: {:?}", cmd);
    task::run(cmd, true)?;
    let output = gradle
        .join("app")
        .join("build")
        .join("outputs")
        .join(match format {
            Format::Aab => "bundle",
            Format::Apk => "apk",
            _ => unreachable!(),
        })
        .join(opt.to_string())
        .join(match (format, opt) {
            (Format::Apk, Opt::Debug) => "app-debug.apk",
            (Format::Apk, Opt::Release) => {
                if gradle.join("app").join("release-key.jks").exists() {
                    "app-release.apk"
                } else {
                    "app-release-unsigned.apk"
                }
            }
            (Format::Aab, Opt::Debug) => "app-debug.aab",
            (Format::Aab, Opt::Release) => "app-release.aab",
            _ => unreachable!(),
        });
    std::fs::copy(output, out)?;
    Ok(())
}

fn create_gradle_wrapper(gradle_dir: &Path) -> Result<()> {
    let wrapper_dir = gradle_dir.join("gradle").join("wrapper");
    std::fs::create_dir_all(&wrapper_dir)?;

    let wrapper_properties = r#"distributionBase=GRADLE_USER_HOME
distributionPath=wrapper/dists
distributionUrl=https\://services.gradle.org/distributions/gradle-8.10.2-bin.zip
networkTimeout=10000
validateDistributionUrl=true
zipStoreBase=GRADLE_USER_HOME
zipStorePath=wrapper/dists
"#;
    std::fs::write(wrapper_dir.join("gradle-wrapper.properties"), wrapper_properties)?;

    let gradlew_script = r#"#!/bin/sh

##############################################################################
# Gradle start up script for UN*X
##############################################################################

# Attempt to set APP_HOME
# Resolve links: $0 may be a link
PRG="$0"
# Need this for relative symlinks.
while [ -h "$PRG" ] ; do
    ls=`ls -ld "$PRG"`
    link=`expr "$ls" : '.*-> \(.*\)$'`
    if expr "$link" : '/.*' > /dev/null; then
        PRG="$link"
    else
        PRG=`dirname "$PRG"`"/$link"
    fi
done
SAVED="`pwd`"
cd "`dirname \"$PRG\"`/" >/dev/null
APP_HOME="`pwd -P`"
cd "$SAVED" >/dev/null

APP_NAME="Gradle"
APP_BASE_NAME=`basename "$0"`

# Add default JVM options here. You can also use JAVA_OPTS and GRADLE_OPTS to pass JVM options to this script.
DEFAULT_JVM_OPTS='"-Xmx64m" "-Xms64m"'

# Use the maximum available, or set MAX_FD != -1 to use that value.
MAX_FD="maximum"

warn () {
    echo "$*"
}

die () {
    echo
    echo "$*"
    echo
    exit 1
}

# OS specific support (must be 'true' or 'false').
cygwin=false
msys=false
darwin=false
nonstop=false
case "`uname`" in
  CYGWIN* )
    cygwin=true
    ;;
  Darwin* )
    darwin=true
    ;;
  MINGW* )
    msys=true
    ;;
  NONSTOP* )
    nonstop=true
    ;;
esac

CLASSPATH=$APP_HOME/gradle/wrapper/gradle-wrapper.jar

# Determine the Java command to use to start the JVM.
if [ -n "$JAVA_HOME" ] ; then
    if [ -x "$JAVA_HOME/jre/sh/java" ] ; then
        # IBM's JDK on AIX uses strange locations for the executables
        JAVACMD="$JAVA_HOME/jre/sh/java"
    else
        JAVACMD="$JAVA_HOME/bin/java"
    fi
    if [ ! -x "$JAVACMD" ] ; then
        die "ERROR: JAVA_HOME is set to an invalid directory: $JAVA_HOME

Please set the JAVA_HOME variable in your environment to match the
location of your Java installation."
    fi
else
    JAVACMD="java"
    which java >/dev/null 2>&1 || die "ERROR: JAVA_HOME is not set and no 'java' command could be found in your PATH.

Please set the JAVA_HOME variable in your environment to match the
location of your Java installation."
fi

# Increase the maximum file descriptors if we can.
if [ "$cygwin" = "false" -a "$darwin" = "false" -a "$nonstop" = "false" ] ; then
    MAX_FD_LIMIT=`ulimit -H -n`
    if [ $? -eq 0 ] ; then
        if [ "$MAX_FD" = "maximum" -o "$MAX_FD" = "max" ] ; then
            MAX_FD="$MAX_FD_LIMIT"
        fi
        ulimit -n $MAX_FD
        if [ $? -ne 0 ] ; then
            warn "Could not set maximum file descriptor limit: $MAX_FD"
        fi
    else
        warn "Could not query maximum file descriptor limit: $MAX_FD_LIMIT"
    fi
fi

# For Darwin, add options to specify how the application appears in the dock
if [ "$darwin" = "true" ]; then
    GRADLE_OPTS="$GRADLE_OPTS \"-Xdock:name=$APP_NAME\" \"-Xdock:icon=$APP_HOME/media/gradle.icns\""
fi

# For Cygwin or MSYS, switch paths to Windows format before running java
if [ "$cygwin" = "true" -o "$msys" = "true" ] ; then
    APP_HOME=`cygpath --path --mixed "$APP_HOME"`
    CLASSPATH=`cygpath --path --mixed "$CLASSPATH"`
    
    JAVACMD=`cygpath --unix "$JAVACMD"`

    # We build the pattern for arguments to be converted via cygpath
    ROOTDIRSRAW=`find -L / -maxdepth 1 -mindepth 1 -type d 2>/dev/null`
    SEP=""
    for dir in $ROOTDIRSRAW ; do
        ROOTDIRS="$ROOTDIRS$SEP$dir"
        SEP="|"
    done
    OURCYGPATTERN="(^($ROOTDIRS))"
    # Add a user-defined pattern to the cygpath arguments
    if [ "$GRADLE_CYGPATTERN" != "" ] ; then
        OURCYGPATTERN="$OURCYGPATTERN|($GRADLE_CYGPATTERN)"
    fi
    # Now convert the arguments - kludge to limit ourselves to /bin/sh
    i=0
    for arg in "$@" ; do
        CHECK=`echo "$arg"|egrep -c "$OURCYGPATTERN" -`
        CHECK2=`echo "$arg"|egrep -c "^-"`                                 ### Determine if an option

        if [ $CHECK -ne 0 ] && [ $CHECK2 -eq 0 ] ; then                    ### Added a condition
            eval `echo args$i`=`cygpath --path --ignore --mixed "$arg"`
        else
            eval `echo args$i`="\"$arg\""
        fi
        i=`expr $i + 1`
    done
    case $i in
        0) set -- ;;
        1) set -- "$args0" ;;
        2) set -- "$args0" "$args1" ;;
        3) set -- "$args0" "$args1" "$args2" ;;
        4) set -- "$args0" "$args1" "$args2" "$args3" ;;
        5) set -- "$args0" "$args1" "$args2" "$args3" "$args4" ;;
        6) set -- "$args0" "$args1" "$args2" "$args3" "$args4" "$args5" ;;
        7) set -- "$args0" "$args1" "$args2" "$args3" "$args4" "$args5" "$args6" ;;
        8) set -- "$args0" "$args1" "$args2" "$args3" "$args4" "$args5" "$args6" "$args7" ;;
        9) set -- "$args0" "$args1" "$args2" "$args3" "$args4" "$args5" "$args6" "$args7" "$args8" ;;
    esac
fi

# Escape application args
save () {
    for i do printf %s\\n "$i" | sed "s/'/'\\\\''/g;1s/^/'/;\$s/\$/' \\\\/" ; done
    echo " "
}
APP_ARGS=`save "$@"`

# Collect all arguments for the java command
set -- $DEFAULT_JVM_OPTS $JAVA_OPTS $GRADLE_OPTS "\"-Dorg.gradle.appname=$APP_BASE_NAME\"" -classpath "\"$CLASSPATH\"" org.gradle.wrapper.GradleWrapperMain "$APP_ARGS"

exec "$JAVACMD" "$@"
"#;
    std::fs::write(gradle_dir.join("gradlew"), gradlew_script)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(gradle_dir.join("gradlew"))?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(gradle_dir.join("gradlew"), perms)?;
    }

    download_gradle_wrapper_jar(&wrapper_dir)?;

    Ok(())
}

fn download_gradle_wrapper_jar(wrapper_dir: &Path) -> Result<()> {
    let jar_path = wrapper_dir.join("gradle-wrapper.jar");
    
    let status = Command::new("curl")
        .args([
            "-L",
            "-o",
            jar_path.to_str().context("invalid path")?,
            "https://github.com/gradle/gradle/raw/v8.10.2/gradle/wrapper/gradle-wrapper.jar"
        ])
        .status()?;

    if !status.success() {
        std::fs::write(&jar_path, b"")?;
    }

    Ok(())
}
