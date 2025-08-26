#!/bin/sh

RAW_VERSION=$(cat ui/Cargo.toml | grep version | head -n 1 | sed "s/.*\"\(.*\)\".*/\1/")
VERSION="$RAW_VERSION"
BUNDLE_VERSION="$VERSION"

TARGET=aarch64-apple-ios
MAKE_IPA=no

if [ "$1" == "--simulator" ]; then
	TARGET=x86_64-apple-ios
elif [ "$1" == "--ipa" ]; then
	MAKE_IPA=yes
	SECRETS_DIR="$2"
	BUNDLE_VERSION="$3"
fi

cargo build --release -p nertsio_ui --target $TARGET

APP_DIR=target/$TARGET/dist/nertsio.app

if [[ "$RAW_VERSION" == *-* ]]; then
	VERSION="${RAW_VERSION%-*}"
fi

rm -rf "$APP_DIR"

mkdir -p "$APP_DIR"

sed "s/%VERSION%/$VERSION/g" < ui/res/platform/ios/Info.plist.in | sed "s/%BUNDLE_VERSION%/$BUNDLE_VERSION/g" > "$APP_DIR"/Info.plist
cp target/ios_res/nertsio.iconset/* "$APP_DIR"/
cp target/$TARGET/release/nertsio_ui "$APP_DIR"/
echo -n "APPL????" > "$APP_DIR"/PkgInfo

if [ "$MAKE_IPA" == "yes" ]; then
	# Based on https://github.com/marysaka/simple_rust_ios_app

	IPA_DIR=target/$TARGET/dist/ipacontent

	rm -rf target/$TARGET/dist/nertsio.ipa
	rm -rf "$IPA_DIR"

	mkdir -p "$IPA_DIR"/Payload

	cp -r "$APP_DIR" "$IPA_DIR"/Payload/nertsio.app
	cp "$SECRETS_DIR"/embedded.mobileprovision "$IPA_DIR"/Payload/nertsio.app

	rcodesign sign --p12-file "$SECRETS_DIR"/private.p12 -e ui/res/platform/ios/nertsio.entitlements "$IPA_DIR"/Payload/nertsio.app

	pushd "$IPA_DIR"
	zip -r ../nertsio.ipa *
	popd
fi
