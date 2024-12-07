#!/bin/sh

cargo build --release -p nertsio_ui --target x86_64-apple-darwin

APP_DIR=target/x86_64-apple-darwin/dist/nertsio.app
VERSION=$(cat ui/Cargo.toml | grep version | head -n 1 | sed "s/.*\"\(.*\)\".*/\1/")

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR"

mkdir "$APP_DIR/Contents"
mkdir "$APP_DIR/Contents/Resources"
mkdir "$APP_DIR/Contents/MacOS"

sed "s/%VERSION%/$VERSION/g" < ui/res/platform/macos/Info.plist.in > "$APP_DIR"/Contents/Info.plist
iconutil -c icns --output "$APP_DIR/Contents/Resources/nertsio.icns" target/mac_res/nertsio.iconset
cp target/x86_64-apple-darwin/release/nertsio_ui "$APP_DIR"/Contents/MacOS/

( cd target/x86_64-apple-darwin/dist; bsdtar -caf nertsio.app.zip nertsio.app )
