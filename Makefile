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

.PHONY: all release release-profile emulator-toolchain emulator-setup emulator-run emulator-build emulator-sync emulator-dev emulator-deck lint lint-parsers

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

# Lint: clippy plus the ast-grep rules in lint/ (see sgconfig.yml). ast-grep
# comes from `cargo install ast-grep --locked`, the Slint parser build needs
# the tree-sitter CLI (`npm i -g tree-sitter-cli`).
AST_GREP ?= $(shell command -v ast-grep 2>/dev/null || echo $(HOME)/.cargo/bin/ast-grep)
TREE_SITTER ?= tree-sitter
SLINT_GRAMMAR_URL = https://github.com/slint-ui/tree-sitter-slint
SLINT_GRAMMAR_REV = b494447aeb10e540b9f0fbde08748c052859e42d
SLINT_GRAMMAR_DIR = target/ast-grep/tree-sitter-slint
SLINT_PARSER = target/ast-grep/slint.so

$(SLINT_PARSER):
	rm -rf $(SLINT_GRAMMAR_DIR)
	git clone --quiet $(SLINT_GRAMMAR_URL) $(SLINT_GRAMMAR_DIR)
	git -C $(SLINT_GRAMMAR_DIR) checkout --quiet $(SLINT_GRAMMAR_REV)
	cd $(SLINT_GRAMMAR_DIR) && $(TREE_SITTER) build --output ../slint.so

lint-parsers: $(SLINT_PARSER)

lint: lint-parsers
	cargo clippy -p pbanki-core -p pbanki-desktop --all-targets
	cargo clippy -p pbanki --features sdk-6-8 --all-targets
	$(AST_GREP) test --custom-languages allow --skip-snapshot-tests
	$(AST_GREP) scan --custom-languages allow
