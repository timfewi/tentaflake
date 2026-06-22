#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────
# hermes-firstboot — First-boot setup for live agent ISO
#
# Runs on TTY1 auto-login. Three modes:
#   1. USB HERMES_ENV detected → silent, auto-config
#   2. No USB → interactive TUI wizard
#   3. Already configured → skip (stamp file)
# ────────────────────────────────────────────────────────────
set -euo pipefail

ENV_DIR="/run/hermes"
STAMP="${ENV_DIR}/.configured"
AGENTS=(default research)

# ── Skip if already configured this session ──
if [ -f "$STAMP" ]; then
	exit 0
fi

mkdir -p "$ENV_DIR"

# ── STEP 1: Try USB HERMES_ENV auto-detection ──
USB_DEV=$(blkid -l -o device -t LABEL=HERMES_ENV 2>/dev/null || true)
if [ -n "$USB_DEV" ]; then
	mkdir -p /mnt/hermes-env
	mount "$USB_DEV" /mnt/hermes-env 2>/dev/null || true
	FOUND=0
	for agent in "${AGENTS[@]}"; do
		if [ -f "/mnt/hermes-env/${agent}.env" ]; then
			cp "/mnt/hermes-env/${agent}.env" "${ENV_DIR}/${agent}.env"
			chmod 600 "${ENV_DIR}/${agent}.env"
			FOUND=$((FOUND + 1))
		fi
	done
	umount /mnt/hermes-env 2>/dev/null || true
	rmdir /mnt/hermes-env 2>/dev/null || true

	if [ "$FOUND" -gt 0 ]; then
		echo ""
		echo "  ✓ USB HERMES_ENV: loaded $FOUND agent env files"
		echo ""
		touch "$STAMP"
		systemctl restart docker-hermes-default docker-hermes-research 2>/dev/null || true
		exit 0
	fi
fi

# ── STEP 2: Interactive TUI wizard ──
exec </dev/tty1 >/dev/tty1 2>&1

clear
cat <<"EOF"
╔══════════════════════════════════════════════════╗
║     Tentaflake — Live ISO        ║
╠══════════════════════════════════════════════════╣
║  Enter API keys to activate your agents.        ║
║  (Press Cancel to skip — agents won't start)    ║
╚══════════════════════════════════════════════════╝
EOF
echo ""

# OpenRouter API key (required)
OR_KEY=$(dialog --stdout --title "API Keys" \
	--inputbox "OpenRouter API Key (required for model access):" 8 60 "" 2>&1) || OR_KEY=""

# Telegram bot token (optional)
TG_KEY=$(dialog --stdout --title "API Keys" \
	--inputbox "Telegram Bot Token (optional, press Enter to skip):" 8 60 "" 2>&1) || TG_KEY=""

# Firecrawl API key (optional)
FC_KEY=$(dialog --stdout --title "API Keys" \
	--inputbox "Firecrawl API Key (optional, needed for web search):" 8 60 "" 2>&1) || FC_KEY=""

# Groq API key (optional, needed for STT)
GQ_KEY=$(dialog --stdout --title "API Keys" \
	--inputbox "Groq API Key (optional, needed for speech-to-text):" 8 60 "" 2>&1) || GQ_KEY=""

# ── Write env files ──
for agent in "${AGENTS[@]}"; do
	{
		echo "OPENROUTER_API_KEY=${OR_KEY}"
		[ -n "$TG_KEY" ] && echo "TELEGRAM_BOT_TOKEN=${TG_KEY}"
		[ -n "$FC_KEY" ] && echo "FIRECRAWL_API_KEY=${FC_KEY}"
		[ -n "$GQ_KEY" ] && echo "GROQ_API_KEY=${GQ_KEY}"
	} >"${ENV_DIR}/${agent}.env"
	chmod 600 "${ENV_DIR}/${agent}.env"
done

touch "$STAMP"

echo ""
echo "  ✓ API keys saved. Starting agents..."
echo "  (Docker pulls the Hermes image on first start — ~2GB, ~1-2 min)"
echo ""

# Restart containers to pick up env files
systemctl restart docker-hermes-default docker-hermes-research 2>/dev/null || true

echo ""
cat <<"EOF"
╔══════════════════════════════════════════════════╗
║  Live system ready!                              ║
║                                                  ║
║  • Agents starting in background                 ║
║  • Piper TTS at http://localhost:5001/v1         ║
║  • Tailscale: sudo tailscale up                  ║
║                                                  ║
║  To install to disk:                             ║
║    /etc/tentaflake/install.sh     ║
║                                                  ║
║  To re-enter setup: rm /run/hermes/.configured   ║
╚══════════════════════════════════════════════════╝
EOF
echo ""
