obj-m += minifuse_mod.o

# DKMS provides KERNELRELEASE. If not set, fallback to the currently running kernel.
KVER ?= $(if $(KERNELRELEASE),$(KERNELRELEASE),$(shell uname -r))
KDIR ?= /lib/modules/$(KVER)/build

# Check if the kernel was built using LLVM/Clang
IS_LLVM := $(shell test -f $(KDIR)/.config && grep -q "CONFIG_CC_IS_CLANG=y" $(KDIR)/.config && echo 1 || echo 0)

ifeq ($(IS_LLVM), 1)
	KBUILD_FLAGS := LLVM=1 CC=clang
else
	KBUILD_FLAGS :=
endif

all:
	$(MAKE) $(KBUILD_FLAGS) -C $(KDIR) M=$(PWD) modules

clean:
	$(MAKE) $(KBUILD_FLAGS) -C $(KDIR) M=$(PWD) clean

# Installs the module with DKMS, which rebuilds it for every new kernel. Once installed it loads
# by itself whenever a MiniFuse is connected. Run as root from the kmod/ directory.
DKMS_NAME := minifuse_mod
DKMS_VERSION = $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(CURDIR)/../Cargo.toml 2>/dev/null)
DKMS_SRC = /usr/src/$(DKMS_NAME)-$(DKMS_VERSION)

install:
	@test -n "$(DKMS_VERSION)" || { echo "Cannot read the version from ../Cargo.toml"; exit 1; }
	-dkms remove $(DKMS_NAME)/$(DKMS_VERSION) --all >/dev/null 2>&1
	rm -rf $(DKMS_SRC)
	install -Dm644 -t $(DKMS_SRC) minifuse_mod.c Makefile LICENSE
	sed 's/^PACKAGE_VERSION=.*/PACKAGE_VERSION="$(DKMS_VERSION)"/' dkms.conf > $(DKMS_SRC)/dkms.conf
	dkms install $(DKMS_NAME)/$(DKMS_VERSION)
	@# Swap in the new build now; this fails while mf-gui has the device open
	-modprobe -r $(DKMS_NAME)
	modprobe $(DKMS_NAME)

uninstall:
	-modprobe -r $(DKMS_NAME)
	-dkms remove $(DKMS_NAME)/$(DKMS_VERSION) --all
	rm -rf $(DKMS_SRC)

.PHONY: all clean install uninstall
