# Makefile — TokenMon for Windows (Rust cargo workspace in windows/)
#
# GNU make 4.x. Run it from this directory:
#
#   cd windows
#   make            # list every target
#   make run        # release build, then launch TokenMon.exe   (Windows)
#   make install    # build the MSI and install it for this user (Windows)
#
# Getting make on Windows (Git for Windows is already required):
#   winget install ezwinports.make      or      choco install make
# Any shell works (PowerShell, cmd, Git Bash): on Windows every recipe runs in
# Git for Windows' bash, found from `where git` (or set GIT_BASH to its
# bin\bash.exe), so the recipes are the same POSIX sh on every OS.
#
# Never runs installer/smoke-test.ps1: that script is for throwaway CI
# runners and deletes TokenMon's registry entries.

.DEFAULT_GOAL := help

# Shell: Git for Windows' bash on Windows, /bin/sh elsewhere

ifeq ($(OS),Windows_NT)
IS_WINDOWS := 1
EXE        := .exe
_empty :=
_space := $(_empty) $(_empty)
# Make splits words on spaces ("C:\Program Files"). Paths below keep '?' in
# place of each space: one word to make, and still matching the space in
# $(wildcard).
_q = $(subst $(_space),?,$(subst \,/,$(1)))
ifeq ($(strip $(GIT_BASH)),)
# where.exe prints one path per line; $(shell) joins the lines with spaces.
_where    := $(subst .exe$(_space),.exe|,$(subst \,/,$(shell where git.exe)))
_git_exe  := $(firstword $(subst |,$(_space),$(subst $(_space),?,$(_where))))
# <root>\cmd\git.exe, <root>\bin\git.exe or <root>\mingw64\bin\git.exe
_git_root := $(patsubst %/cmd/git.exe,%,$(patsubst %/bin/git.exe,%,$(patsubst %/mingw64/bin/git.exe,%,$(_git_exe))))
# <root>\bin\bash.exe is the launcher that puts /usr/bin on PATH; the
# default install folders are the fallback when git.exe is not on PATH.
_bash_candidates := $(if $(_git_root),$(_git_root)/bin/bash.exe) \
	$(if $(ProgramW6432),$(call _q,$(ProgramW6432))/Git/bin/bash.exe) \
	$(if $(ProgramFiles),$(call _q,$(ProgramFiles))/Git/bin/bash.exe) \
	$(if $(LOCALAPPDATA),$(call _q,$(LOCALAPPDATA))/Programs/Git/bin/bash.exe)
GIT_BASH := $(subst ?,$(_space),$(firstword $(foreach c,$(_bash_candidates),$(if $(wildcard $(c)),$(c)))))
endif
ifeq ($(strip $(GIT_BASH)),)
$(error Git for Windows' bash.exe was not found (looked next to `where git` and in the default install folders). Install Git for Windows (winget install Git.Git) and open a new terminal, or point GIT_BASH at it: make GIT_BASH="C:/Program Files/Git/bin/bash.exe")
endif
SHELL := $(GIT_BASH)
# Sub-makes (make run -> make release) skip the search.
export GIT_BASH
PYTHON ?= python
else
IS_WINDOWS :=
EXE        :=
SHELL  := /bin/sh
PYTHON ?= python3
endif
.SHELLFLAGS := -ec

# Configuration

APP_NAME    := TokenMon
TARGET      := x86_64-pc-windows-msvc
CARGO       ?= cargo
LOCKED      := --locked
TOOLS_DIR   := .tools
DIST_DIR    := target/wix
MSI_UI      ?= /qb
UPGRADE_CODE := {97EEF0DA-B00F-4527-93D7-EF335558A346}

# Pinned like .github/workflows/windows-release.yml (same versions, URLs and
# SHA-256s; keep them in step).
WIX_VERSION       := 3.14.1
WIX_URL           := https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip
WIX_SHA256        := 6ac824e1642d6f7277d0ed7ea09411a508f6116ba6fae0aa5f2c7daa2ff43d31
CARGO_WIX_VERSION := 0.3.9
CARGO_WIX_URL     := https://static.crates.io/crates/cargo-wix/cargo-wix-$(CARGO_WIX_VERSION).crate
CARGO_WIX_SHA256  := 7023fb5f51309588c35ef9323b58a805e6a9bad4be4d4e3054264fea9a1cdbad

WIX_DIR   := $(TOOLS_DIR)/wix314
CANDLE    := $(WIX_DIR)/candle.exe
CARGO_WIX := $(TOOLS_DIR)/cargo-wix/bin/cargo-wix$(EXE)

# On Windows the app builds for the release target (the same paths CI and
# cargo-wix use); elsewhere `build` and `release` build the native stub.
ifeq ($(IS_WINDOWS),1)
TARGET_FLAG := --target $(TARGET)
BIN_DIR     := target/$(TARGET)
else
TARGET_FLAG :=
BIN_DIR     := target
endif
DEBUG_EXE   := $(BIN_DIR)/debug/$(APP_NAME)$(EXE)
RELEASE_EXE := $(BIN_DIR)/release/$(APP_NAME)$(EXE)

# [workspace.package] version (what the MSI and the windows-v tag use)
VERSION := $(shell $(PYTHON) -I installer/package.py version 2>/dev/null | tr -d '\r')
# Release file names from package.py (msi=, zip=, sums=), read when needed.
ASSET_NAMES = $(shell $(PYTHON) -I installer/package.py asset-names $(VERSION) | tr -d '\r')
asset = $(DIST_DIR)/$(patsubst $(1)=%,%,$(filter $(1)=%,$(ASSET_NAMES)))
MSI  = $(call asset,msi)
ZIP  = $(call asset,zip)
SUMS = $(call asset,sums)

# Colors (NO_COLOR=1 turns them off)
ifeq ($(NO_COLOR),)
CYAN  := \033[36m
GREEN := \033[32m
YELL  := \033[33m
BOLD  := \033[1m
RESET := \033[0m
endif

define say
	@printf "$(CYAN)==>$(RESET) %s\n" "$(1)"
endef

define ok
	@printf "$(GREEN)ok$(RESET)  %s\n" "$(1)"
endef

# Fails the target on macOS/Linux with a pointer to what does work there.
define windows_only
	@if [ "$(IS_WINDOWS)" != 1 ]; then \
		printf "$(YELL)make $@ needs Windows$(RESET) (it builds, packages or runs TokenMon.exe).\n" >&2; \
		printf "On macOS/Linux use: make build test lint xcheck fuzz-check parity\n" >&2; \
		exit 1; \
	fi
endef

.PHONY: help version build release run run-debug smoke test lint fmt fuzz-check parity xcheck \
	tools installer zip dist install uninstall clean distclean

##@ Help

help: ## Show this help (default)
	@printf "\n$(BOLD)TokenMon for Windows$(RESET) — system-tray usage monitor (Rust)\n"
	@printf "Version: %s\n" "$(VERSION)"
	@awk 'BEGIN { FS = ":.*## " } \
		/^##@/ { printf "\n$(BOLD)%s$(RESET)\n", substr($$0, 5); next } \
		/^[a-zA-Z_-]+:.*## / { printf "  $(CYAN)%-12s$(RESET) %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@printf "\n[W] Windows only. The rest also works on macOS and Linux.\n"
	@printf "\nExamples:\n"
	@printf "  make run              # release build, then start TokenMon\n"
	@printf "  make install          # MSI -> %%LOCALAPPDATA%%\\\\Programs\\\\TokenMon\n"
	@printf "  make test lint        # the gate before every push\n"
	@printf "  make install MSI_UI=/qn   # silent install\n"
	@printf "\n"

version: ## Print the workspace version
	@printf "%s\n" "$(VERSION)"

##@ Build and run

build: ## Debug build of TokenMon.exe
	$(call say,Building $(APP_NAME) [debug])
	$(CARGO) build $(LOCKED) -p tokenmon-app $(TARGET_FLAG)
	$(call ok,$(DEBUG_EXE))

release: ## Release build of TokenMon.exe
	$(call say,Building $(APP_NAME) [release])
	$(CARGO) build $(LOCKED) --release -p tokenmon-app $(TARGET_FLAG)
	$(call ok,$(RELEASE_EXE))

# Stops a running TokenMon (single instance) and starts $(1) detached.
define launch
	$(call windows_only)
	@taskkill //IM $(APP_NAME).exe >/dev/null 2>&1 || true; \
	i=0; while tasklist //FI "IMAGENAME eq $(APP_NAME).exe" 2>/dev/null | grep -qi "$(APP_NAME).exe" && [ $$i -lt 25 ]; do \
		sleep 0.2; i=$$((i + 1)); \
	done; \
	taskkill //F //IM $(APP_NAME).exe >/dev/null 2>&1 || true
	@cmd //c start "" "$$(cygpath -w "$(1)")"
	$(call ok,Launched $(1) (look for the tray icon))
endef

run: ## [W] Release build, then launch TokenMon.exe
	$(call windows_only)
	@"$(MAKE)" --no-print-directory release
	$(call launch,$(RELEASE_EXE))

run-debug: ## [W] Debug build, then launch TokenMon.exe
	$(call windows_only)
	@"$(MAKE)" --no-print-directory build
	$(call launch,$(DEBUG_EXE))

smoke: ## [W] Release build, then TokenMon.exe --smoke (headless, one refresh)
	$(call windows_only)
	@"$(MAKE)" --no-print-directory release
	"$(RELEASE_EXE)" --smoke
	$(call ok,smoke passed)

##@ Quality

test: ## cargo test --workspace
	$(CARGO) test $(LOCKED) --workspace

lint: ## fmt check, clippy -D warnings, cargo deny (if installed)
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy $(LOCKED) --workspace --all-targets -- -D warnings
	@if $(CARGO) deny --version >/dev/null 2>&1; then \
		echo "$(CARGO) deny --manifest-path Cargo.toml check advisories bans licenses sources"; \
		$(CARGO) deny --manifest-path Cargo.toml check advisories bans licenses sources; \
	else \
		printf "$(YELL)cargo-deny not installed; skipped$(RESET) (cargo install cargo-deny --locked)\n"; \
	fi
	$(call ok,lint clean)

fmt: ## cargo fmt --all
	$(CARGO) fmt --all

fuzz-check: ## Type-check the fuzz crate on stable (cargo check fuzz --bins)
	$(CARGO) check --manifest-path fuzz/Cargo.toml --bins

parity: ## Check the Swift-to-Rust test parity ledger (docs/PARITY.md)
	$(PYTHON) -I scripts/parity/check_parity.py

xcheck: ## clippy for x86_64-pc-windows-msvc (cargo-xwin on macOS/Linux)
ifeq ($(IS_WINDOWS),1)
	$(CARGO) clippy $(LOCKED) --workspace --all-targets --target $(TARGET) -- -D warnings
else
	@$(CARGO) xwin --version >/dev/null 2>&1 || { \
		printf "cargo-xwin is not installed: rustup target add $(TARGET) && cargo install cargo-xwin --locked\n" >&2; exit 1; }
	XWIN_ACCEPT_LICENSE=1 $(CARGO) xwin clippy $(LOCKED) --workspace --all-targets --target $(TARGET) -- -D warnings
endif

##@ Package and install

tools: ## [W] WiX 3.14.1 + cargo-wix 0.3.9 into .tools/ (pinned, SHA-256 checked)
	$(call windows_only)
	@"$(MAKE)" --no-print-directory $(CANDLE) $(CARGO_WIX)
	$(call ok,WiX $(WIX_VERSION) and cargo-wix $(CARGO_WIX_VERSION) in $(TOOLS_DIR)/)

# sha256 <file> <expected>: fail unless the file hashes to <expected>.
SHA256_CHECK = $(PYTHON) -I -c 'import hashlib, sys; d = hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest(); sys.exit(0 if d == sys.argv[2] else "SHA-256 mismatch for %s: got %s, want %s" % (sys.argv[1], d, sys.argv[2]))'

$(CANDLE):
	$(call windows_only)
	$(call say,Downloading WiX Toolset $(WIX_VERSION))
	@rm -rf "$(WIX_DIR)" "$(WIX_DIR).tmp" && mkdir -p "$(WIX_DIR).tmp"
	curl -fsSL --retry 3 "$(WIX_URL)" -o "$(WIX_DIR).tmp/wix.zip"
	$(SHA256_CHECK) "$(WIX_DIR).tmp/wix.zip" "$(WIX_SHA256)"
	$(PYTHON) -I -m zipfile -e "$(WIX_DIR).tmp/wix.zip" "$(WIX_DIR).tmp/bin"
	@banner=$$("$(WIX_DIR).tmp/bin/candle.exe" "-?" || true); \
	printf '%s\n' "$$banner" | head -n 1; \
	printf '%s\n' "$$banner" | grep -qF "version $(WIX_VERSION)"
	@mv "$(WIX_DIR).tmp/bin" "$(WIX_DIR)" && rm -rf "$(WIX_DIR).tmp"

# Built from the crates.io source with its own Cargo.lock, like the release
# workflow. The source is unpacked outside windows/ so cargo does not take
# it for a member of this workspace.
$(CARGO_WIX):
	$(call windows_only)
	$(call say,Building cargo-wix $(CARGO_WIX_VERSION))
	@tmp=$$(mktemp -d) && trap 'rm -rf "$$tmp"' EXIT && \
	curl -fsSL --retry 3 "$(CARGO_WIX_URL)" -o "$$tmp/cargo-wix.crate" && \
	$(SHA256_CHECK) "$$tmp/cargo-wix.crate" "$(CARGO_WIX_SHA256)" && \
	$(PYTHON) -I -m tarfile -e "$$(cygpath -w "$$tmp/cargo-wix.crate")" "$$(cygpath -w "$$tmp")" && \
	rm -rf "$(TOOLS_DIR)/cargo-wix" && \
	$(CARGO) install --locked --path "$$(cygpath -w "$$tmp/cargo-wix-$(CARGO_WIX_VERSION)")" \
		--root "$$(cygpath -w "$$PWD/$(TOOLS_DIR)/cargo-wix")" && \
	"$(CARGO_WIX)" wix --version | grep -F "$(CARGO_WIX_VERSION)"

installer: ## [W] Per-user MSI: target/wix/tokenmon-windows-<version>-x64.msi
	$(call windows_only)
	@"$(MAKE)" --no-print-directory release tools
	$(call say,Building $(MSI))
	@mkdir -p "$(DIST_DIR)"
	"$(CARGO_WIX)" wix -p tokenmon-app --no-build --nocapture --target $(TARGET) \
		--bin-path "$$(cygpath -w "$$PWD/$(WIX_DIR)")" \
		--include "$$(cygpath -w "$$PWD/installer/main.wxs")" \
		--output "$$(cygpath -w "$$PWD/$(MSI)")"
	@test -f "$(MSI)"
	$(call ok,$(MSI))

zip: ## [W] Portable zip: target/wix/tokenmon-windows-<version>-x64.zip
	$(call windows_only)
	@"$(MAKE)" --no-print-directory release
	@mkdir -p "$(DIST_DIR)"
	$(PYTHON) -I installer/package.py zip --exe "$(RELEASE_EXE)" --license ../LICENSE \
		--readme installer/portable-README.txt --out "$(ZIP)"
	$(call ok,$(ZIP))

dist: ## [W] MSI + zip + SHA256SUMS in target/wix/ (unsigned release files)
	$(call windows_only)
	@"$(MAKE)" --no-print-directory installer zip
	cd "$(DIST_DIR)" && $(PYTHON) -I ../../installer/package.py sums --out "$(notdir $(SUMS))" \
		"$(notdir $(MSI))" "$(notdir $(ZIP))" && sha256sum -c "$(notdir $(SUMS))"
	$(call ok,$(DIST_DIR)/: $(notdir $(MSI)) $(notdir $(ZIP)) $(notdir $(SUMS)))

# MSYS_NO_PATHCONV=1 keeps Git Bash from turning /i, /x and /qb into paths.
install: ## [W] Build the MSI and install it for this user (no admin)
	$(call windows_only)
	@"$(MAKE)" --no-print-directory installer
	$(call say,Installing $(MSI))
	MSYS_NO_PATHCONV=1 msiexec /i "$$(cygpath -w "$(MSI)")" $(MSI_UI) /norestart
	@dir=$$(MSYS_NO_PATHCONV=1 reg query 'HKCU\Software\TokenMon\Installer' /v InstallDir 2>/dev/null \
		| tr -d '\r' | sed -n 's/.*InstallDir *REG_SZ *//p'); \
	printf "$(GREEN)ok$(RESET)  Installed $(APP_NAME) $(VERSION) to %s\n" "$${dir:-%LOCALAPPDATA%\\Programs\\TokenMon\\}"; \
	printf "    Start it from the Start menu (TokenMon), or: \"%s$(APP_NAME).exe\"\n" "$$dir"; \
	printf "    Remove it with: make uninstall (your data in %%LOCALAPPDATA%%\\\\TokenMon is kept)\n"

# Finds installed copies through Windows Installer by UpgradeCode, so it also
# removes an MSI from a release download. Falls back to the MSI built here.
# Never touches the user's data in %LOCALAPPDATA%\TokenMon.
uninstall: ## [W] Uninstall the per-user MSI (keeps %LOCALAPPDATA%\TokenMon data)
	$(call windows_only)
	@set -o pipefail; codes=$$(powershell.exe -NoProfile -NonInteractive -Command \
		'$$i = New-Object -ComObject WindowsInstaller.Installer; $$i.GetType().InvokeMember("RelatedProducts", "GetProperty", $$null, $$i, @("$(UPGRADE_CODE)")) | ForEach-Object { [string] $$_ }' \
		| tr -d '\r') || codes=unknown; \
	if [ "$$codes" = unknown ]; then \
		msi="$(MSI)"; \
		if [ ! -f "$$msi" ]; then echo "Could not query Windows Installer and $$msi does not exist; run make installer first." >&2; exit 1; fi; \
		printf "$(CYAN)==>$(RESET) Uninstalling by %s\n" "$$msi"; \
		MSYS_NO_PATHCONV=1 msiexec /x "$$(cygpath -w "$$msi")" $(MSI_UI) /norestart; \
	elif [ -z "$$codes" ]; then \
		echo "$(APP_NAME) is not installed (no product with UpgradeCode $(UPGRADE_CODE))."; \
	else \
		for code in $$codes; do \
			printf "$(CYAN)==>$(RESET) Uninstalling %s\n" "$$code"; \
			MSYS_NO_PATHCONV=1 msiexec /x "$$code" $(MSI_UI) /norestart; \
		done; \
		printf "$(GREEN)ok$(RESET)  Uninstalled. Your data in %%LOCALAPPDATA%%\\\\TokenMon is kept.\n"; \
	fi

##@ Clean

clean: ## cargo clean (also removes target/wix)
	$(CARGO) clean
	@rm -rf "$(DIST_DIR)"

distclean: clean ## clean, plus the downloaded tools in .tools/
	@rm -rf "$(TOOLS_DIR)"
