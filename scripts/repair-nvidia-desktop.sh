#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'EOF'
Usage:
  ./scripts/repair-nvidia-desktop.sh        # repair current 595 NVIDIA desktop stack
  ./scripts/repair-nvidia-desktop.sh --580  # switch back to the conservative 580 stack

This fixes the common KDE/Wayland NVIDIA fallback where KWin uses QPainter
and OpenGL reports Mesa llvmpipe after a driver change.
EOF
  exit 0
fi

series="595"
if [[ "${1:-}" == "--580" ]]; then
  series="580"
elif [[ "${1:-}" == "--595" || -z "${1:-}" ]]; then
  series="595"
else
  echo "Unbekannte Option: ${1:-}" >&2
  echo "Nutze --help fuer Hilfe." >&2
  exit 2
fi

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  exec sudo "$0" "$@"
fi

driver="nvidia-driver-$series"
gl_package="libnvidia-gl-$series"

echo "Repair target: $driver"
echo
apt update
apt install -y "$driver" "$gl_package" nvidia-prime libnvidia-egl-wayland1
apt install -y --reinstall "$driver" "$gl_package" nvidia-prime libnvidia-egl-wayland1

prime-select nvidia
install -d /etc/modprobe.d
printf 'options nvidia-drm modeset=1 fbdev=1\n' > /etc/modprobe.d/nvidia-kms.conf

dkms autoinstall || true
update-initramfs -u

echo
echo "Verification before reboot:"
prime-select query || true
dpkg -V "$gl_package" || true
ls -l /usr/share/glvnd/egl_vendor.d/10_nvidia.json 2>/dev/null || true
ls -l /usr/share/egl/egl_external_platform.d/15_nvidia_gbm.json 2>/dev/null || true

echo
echo "Done. Reboot now:"
echo "  sudo reboot"
