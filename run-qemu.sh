#!/usr/bin/env bash
# This is the build/launch script used by cargo run
set -e

KERNEL_BIN="$1"
IMG="boot.img"

# Create the 64MB FAT32 boot image automatically if it's missing
if [ ! -f "$IMG" ]; then
    dd if=/dev/zero of="$IMG" bs=1M count=64 status=none
    mkfs.vfat -F 32 "$IMG" > /dev/null
    mmd -i "$IMG" ::/EFI ::/EFI/BOOT ::/boot
    mcopy -i "$IMG" limine/BOOTAA64.EFI ::/EFI/BOOT/
    mcopy -i "$IMG" limine.conf ::/
fi

# Update the kernel binary inside the image
mcopy -o -i "$IMG" "$KERNEL_BIN" ::/boot/pulse_controller

# Launch QEMU with UEFI firmware
exec qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a53 \
    -m 512M \
    -bios /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
    -drive file="$IMG",format=raw \
    -nographic
