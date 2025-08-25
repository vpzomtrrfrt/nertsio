#!/bin/sh

TARGET=aarch64-apple-ios

if [ "$1" == "--simulator" ]; then
	TARGET=x86_64-apple-ios
fi

cargo build --release -p nertsio_ui --target $TARGET

APP_DIR=target/$TARGET/dist/nertsio.app
VERSION=$(cat ui/Cargo.toml | grep version | head -n 1 | sed "s/.*\"\(.*\)\".*/\1/")

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR"

sed "s/%VERSION%/$VERSION/g" < ui/res/platform/ios/Info.plist.in > "$APP_DIR"/Info.plist
iconutil -c icns --output "$APP_DIR/nertsio.icns" target/mac_res/nertsio.iconset
cp target/$TARGET/release/nertsio_ui "$APP_DIR"/

( cd target/$TARGET/dist; bsdtar -caf nertsio.app.zip nertsio.app )
