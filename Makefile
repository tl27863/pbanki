all:
	cargo zigbuild --target armv7-unknown-linux-gnueabi.2.23 --profile dev -p pbanki --features=sdk-6-8

release:
	cargo zigbuild --target armv7-unknown-linux-gnueabi.2.23 --profile release -p pbanki --features=sdk-6-8

.PHONY: emulator-toolchain emulator-setup emulator-run emulator-build emulator-sync emulator-dev

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
