# Two device builds, both for firmware 6.10+ (glibc 2.38, the floor of the
# 6.10 and 6.11 libinkview.so):
# - base: B288/B300 devices, soft-float, SDK 6.10 bindings -> target/pocketbook/
# - rk3566: RK3566 devices, hard-float, SDK 6.11 bindings -> target/pocketbook-rk3566/
GLIBC = 2.38

# Zig and cargo-zigbuild come from mise (mise.toml, `mise install` once); add
# their paths so make works without `mise activate` in the shell.
MISE_BIN_PATHS := $(shell mise bin-paths 2>/dev/null | paste -sd: -)
ifneq ($(MISE_BIN_PATHS),)
export PATH := $(MISE_BIN_PATHS):$(PATH)
endif
BASE_TARGET = armv7-unknown-linux-gnueabi
BASE_SDK = sdk-6-10
# softfp: hardware VFP/NEON with the soft-float calling convention (GCC's
# -mfloat-abi=softfp), so the binary still links against the soft-float
# system libraries. The plain gnueabi target emulates all float math in
# software (__aeabi_fmul calls). See notes/performance.md, "Float ABI of the
# PB710 build". `make release BASE_RUSTFLAGS=` builds with software float.
SOFTFP_RUSTFLAGS = -C target-feature=+vfp4,+neon,-soft-float
BASE_RUSTFLAGS ?= $(SOFTFP_RUSTFLAGS)
RK3566_TARGET = armv7-unknown-linux-gnueabihf
RK3566_SDK = sdk-6-11

# $(call bundle,<target triple>,<bundle dir>): stage launcher + binary.
define bundle
	mkdir -p $(2)
	cp target/$(1)/release/pbanki $(2)/pbanki.bin
	cp scripts/pocketbook/pbanki.app $(2)/pbanki.app
	chmod +x $(2)/pbanki.app $(2)/pbanki.bin
	ls -lh $(2)/pbanki.app $(2)/pbanki.bin
endef

all:
	RUSTFLAGS="$(BASE_RUSTFLAGS)" cargo zigbuild --target $(BASE_TARGET).$(GLIBC) --profile dev -p pbanki --features=$(BASE_SDK)

rk3566:
	cargo zigbuild --target $(RK3566_TARGET).$(GLIBC) --profile dev -p pbanki --features=$(RK3566_SDK)

release: FEATURES = $(BASE_SDK)
release-profile: FEATURES = $(BASE_SDK),profile

release release-profile:
	RUSTFLAGS="$(BASE_RUSTFLAGS)" cargo zigbuild --target $(BASE_TARGET).$(GLIBC) --profile release -p pbanki --features=$(FEATURES)
	$(call bundle,$(BASE_TARGET),target/pocketbook)

release-rk3566: FEATURES = $(RK3566_SDK)
release-rk3566-profile: FEATURES = $(RK3566_SDK),profile

release-rk3566 release-rk3566-profile:
	cargo zigbuild --target $(RK3566_TARGET).$(GLIBC) --profile release -p pbanki --features=$(FEATURES)
	$(call bundle,$(RK3566_TARGET),target/pocketbook-rk3566)

# SDK PC emulator (x86-64 InkView in a container): SDK=6.10 (B288, PB700
# profile, default) or SDK=6.11 (RK3566, 1030 profile). See
# scripts/sdk-emulator/lib.sh for PROFILE, RESMODE and EMU_NET.
SDK ?= 6.10
export SDK

sdk-emulator-setup:
	./scripts/sdk-emulator/setup.sh

sdk-emulator-build:
	./scripts/sdk-emulator/build.sh

sdk-emulator: sdk-emulator-build
	./scripts/sdk-emulator/run.sh

.PHONY: all rk3566 release release-profile release-rk3566 release-rk3566-profile sdk-emulator-setup sdk-emulator-build sdk-emulator lint lint-parsers

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
	cargo clippy -p pbanki-desktop --features eink-sim --all-targets
	cargo clippy -p pbanki --features $(BASE_SDK) --all-targets
	cargo clippy -p pbanki --features $(RK3566_SDK) --all-targets
	$(AST_GREP) test --custom-languages allow --skip-snapshot-tests
	$(AST_GREP) scan --custom-languages allow
