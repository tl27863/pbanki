all:
	cargo zigbuild --target armv7-unknown-linux-gnueabi.2.23 --profile dev -p pbanki --features=sdk-6-8

release: FEATURES = sdk-6-8
release-profile: FEATURES = sdk-6-8,profile

release release-profile:
	cargo zigbuild --target armv7-unknown-linux-gnueabi.2.23 --profile release -p pbanki --features=$(FEATURES)
	mkdir -p target/pocketbook
	cp target/armv7-unknown-linux-gnueabi/release/pbanki target/pocketbook/pbanki.bin
	cp scripts/pocketbook/pbanki.app target/pocketbook/pbanki.app
	chmod +x target/pocketbook/pbanki.app target/pocketbook/pbanki.bin
	ls -lh target/pocketbook/pbanki.app target/pocketbook/pbanki.bin

.PHONY: all release release-profile emulator-toolchain emulator-setup emulator-run emulator-build emulator-sync emulator-dev emulator-deck

emulator-toolchain:
	./scripts/emulator/install-toolchain.sh

emulator-setup:
	./scripts/emulator/setup.sh

emulator-run:
	./scripts/emulator/run.sh

emulator-dev:
	./scripts/emulator/dev.sh

emulator-build:
	./scripts/emulator/build-app.sh

emulator-sync:
	./scripts/emulator/sync-app.sh

emulator-deck:
	./scripts/emulator/install-deck.sh
