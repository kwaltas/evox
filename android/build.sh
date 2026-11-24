#!/usr/bin/env bash
# Build an installable arm64 APK of EvoX (no Gradle). Output: android/out/evox.apk
# Needs: rustup target aarch64-linux-android, Android SDK (platform 34,
# build-tools 34.0.0) and NDK 26. Contains no game data: the user loads an ISO.
set -euo pipefail
cd "$(dirname "$0")/.."
SDK=${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}
NDK=${ANDROID_NDK_HOME:-$SDK/ndk/26.1.10909125}
BT=$SDK/build-tools/35.0.0
JAR=$SDK/platforms/android-34/android.jar
API=24
PKG=com.evox.game
TC=$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin
OUT=android/out; W=$OUT/work
rm -rf "$W"; mkdir -p "$W/java/com/evox/game" "$W/java/quad_native" "$W/classes" "$W/apk/lib/arm64-v8a"

# 1. Rust -> libevox_game.so (lib built as cdylib; `quad_main` is the entry)
export CC_aarch64_linux_android=$TC/aarch64-linux-android$API-clang
export AR_aarch64_linux_android=$TC/llvm-ar
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$TC/aarch64-linux-android$API-clang
cargo rustc --release -p evox --lib --target aarch64-linux-android --crate-type cdylib
cp target/aarch64-linux-android/release/libevox_game.so "$W/apk/lib/arm64-v8a/libevox_game.so"

# 2. Java: miniquad's activity + our ISO picker hooks
MQ=$(ls -d ~/.cargo/registry/src/*/miniquad-0.4.11)
python3 -I - "$MQ/java" "$W/java" "$PKG" <<'PY'
import sys
src,dst,pkg=sys.argv[1:4]
m=open(f"{src}/MainActivity.java").read()
m=m.replace("TARGET_PACKAGE_NAME",pkg).replace("LIBRARY_NAME","evox_game")
m=m.replace("//% IMPORTS","import android.os.ParcelFileDescriptor;")
m=m.replace("//% MAIN_ACTIVITY_BODY","""
    public void pickIso() {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                i.addCategory(Intent.CATEGORY_OPENABLE);
                i.setType("*/*");
                startActivityForResult(i, 7701);
            }
        });
    }
""")
m=m.replace("//% MAIN_ACTIVITY_ON_ACTIVITY_RESULT","""
        if (requestCode == 7701) {
            if (resultCode == RESULT_OK && data != null && data.getData() != null) {
                try {
                    ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(data.getData(), "r");
                    EvoxNative.isoPicked(pfd.detachFd());
                } catch (Exception e) {
                    Log.e("EVOX", "open ISO failed", e);
                    EvoxNative.isoPicked(-1);
                }
            } else {
                EvoxNative.isoPicked(-2);
            }
        }
""")
# A game must not dim mid-race.
m=m.replace("super.onCreate(savedInstanceState);","super.onCreate(savedInstanceState);\n        getWindow().addFlags(LayoutParams.FLAG_KEEP_SCREEN_ON);",1)
import os
if os.environ.get("EVOX_DEBUG_HARNESS") == "1":
    # QA only: `am start --es args "--race ALASKA1" --es env "K=V,K2=V2"`.
    m=m.replace("QuadNative.activityOnCreate(this);","""
        try {
            setTurnScreenOn(true);
            setShowWhenLocked(true);
            Intent in = getIntent();
            String a = in.getStringExtra("args");
            if (a != null) android.system.Os.setenv("EVOX_ARGS", a, true);
            String e = in.getStringExtra("env");
            if (e != null) for (String kv : e.split(",")) {
                int i = kv.indexOf('=');
                if (i > 0) android.system.Os.setenv(kv.substring(0, i), kv.substring(i + 1), true);
            }
        } catch (Exception ex) { Log.e("EVOX", "harness", ex); }
        QuadNative.activityOnCreate(this);""",1)
open(f"{dst}/com/evox/game/MainActivity.java","w").write(m)
open(f"{dst}/com/evox/game/EvoxNative.java","w").write(f"package {pkg};\npublic class EvoxNative {{ public static native void isoPicked(int fd); }}\n")
open(f"{dst}/quad_native/QuadNative.java","w").write(open(f"{src}/QuadNative.java").read())
PY

# 3. javac -> d8
javac -nowarn -g:none --release 8 -classpath "$JAR" -d "$W/classes" $(find "$W/java" -name '*.java')
"$BT/d8" --release --min-api $API --lib "$JAR" --output "$W/apk" $(find "$W/classes" -name '*.class')

# 4. Manifest + package + sign
cat > "$W/AndroidManifest.xml" <<MF
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="$PKG"
    android:versionCode="1" android:versionName="0.1.0">
  <uses-sdk android:minSdkVersion="$API" android:targetSdkVersion="34"/>
  <uses-permission android:name="android.permission.INTERNET"/>
  <uses-feature android:glEsVersion="0x00030000" android:required="true"/>
  <application android:label="EvoX" android:extractNativeLibs="true" android:hasCode="true"
      android:allowBackup="false" android:largeHeap="true">
    <activity android:name=".MainActivity" android:exported="true"
        android:theme="@android:style/Theme.NoTitleBar.Fullscreen"
        android:screenOrientation="sensorLandscape"
        android:configChanges="orientation|screenSize|keyboardHidden|keyboard|screenLayout|smallestScreenSize|uiMode">
      <intent-filter>
        <action android:name="android.intent.action.MAIN"/>
        <category android:name="android.intent.category.LAUNCHER"/>
      </intent-filter>
    </activity>
  </application>
</manifest>
MF
"$BT/aapt2" link -o "$W/base.apk" -I "$JAR" --manifest "$W/AndroidManifest.xml" --min-sdk-version $API --target-sdk-version 34
(cd "$W/apk" && zip -qr ../base.apk classes.dex lib)
"$BT/zipalign" -f 4 "$W/base.apk" "$W/aligned.apk"
KS=android/debug.keystore
[ -f $KS ] || keytool -genkeypair -keystore $KS -storepass android -keypass android -alias evox -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=EvoX Debug" >/dev/null 2>&1
"$BT/apksigner" sign --ks $KS --ks-pass pass:android --key-pass pass:android --out $OUT/evox.apk "$W/aligned.apk"
ls -la $OUT/evox.apk
