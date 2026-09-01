#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'EOF'
Usage:
  ./scripts/fix-cuda-nvidia.sh        # conservative desktop path: nvidia-driver-580
  ./scripts/fix-cuda-nvidia.sh --595  # repair/reinstall the currently installed 595 stack
EOF
  exit 0
fi

series="580"
if [[ "${1:-}" == "--595" ]]; then
  series="595"
elif [[ "${1:-}" == "--580" || -z "${1:-}" ]]; then
  series="580"
else
  echo "Unbekannte Option: ${1:-}" >&2
  echo "Nutze --help fuer Hilfe." >&2
  exit 2
fi

driver="nvidia-driver-$series"
gl_package="libnvidia-gl-$series"

echo "Aktueller Kernel: $(uname -r)"
echo
echo "Geladener NVIDIA-Kernel-Treiber:"
cat /proc/driver/nvidia/version 2>/dev/null || echo "kein geladener NVIDIA-Treiber"
echo
echo "nvidia-smi vor dem Fix:"
nvidia-smi 2>&1 || true
echo
echo "Installiere/repariere: $driver + $gl_package + nvidia-prime + sox"
sudo apt update
sudo apt install -y "$driver" "$gl_package" nvidia-prime libnvidia-egl-wayland1 sox
sudo apt install -y --reinstall "$gl_package" nvidia-prime
sudo prime-select nvidia
printf 'options nvidia-drm modeset=1 fbdev=1\n' | sudo tee /etc/modprobe.d/nvidia-kms.conf >/dev/null
sudo dkms autoinstall || true
sudo update-initramfs -u
echo
echo "NVIDIA GL/EGL-Dateien nach dem Repair:"
dpkg -V "$gl_package" || true
ls -l /usr/share/glvnd/egl_vendor.d/10_nvidia.json 2>/dev/null || true
echo
echo "Fertig. Bitte jetzt neu starten:"
echo "  sudo reboot"
echo
echo "Nach dem Reboot prüfen:"
echo "  nvidia-smi"
echo "  prime-select query"
echo "  glxinfo -B | grep -E 'OpenGL vendor|OpenGL renderer|Accelerated'"
echo "  qdbus6 org.kde.KWin /KWin org.kde.KWin.supportInformation | grep 'Compositing Type'"
echo "  backend/.venv/bin/python - <<'PY'"
echo "import torch; print(torch.cuda.is_available()); print(torch.cuda.get_device_name(0) if torch.cuda.is_available() else 'no cuda')"
echo "PY"
