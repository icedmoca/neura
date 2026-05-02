# Chromium Agent Bridge

Local Chrome/Chromium automation bridge modeled after the Firefox Browser Agent Bridge.

## Files

- Extension directory: `~/.kcode/chromium-agent-bridge/extension`
- Packaged zip: `~/.kcode/chromium-agent-bridge/chromium-agent-bridge-extension.zip`
- CLI/server: `~/.kcode/chromium-agent-bridge/chromium-agent-bridge`
- Convenience symlink: `~/.local/bin/chromium-agent-bridge`
- WebSocket server: `ws://127.0.0.1:8767`

## Load into Chrome

Chrome blocks silent extension install from local tools. Load once manually:

1. Open `chrome://extensions`
2. Enable **Developer mode**
3. Click **Load unpacked**
4. Select `~/.kcode/chromium-agent-bridge/extension`

After that, the extension connects to the local bridge automatically.

## Commands

```bash
chromium-agent-bridge status
chromium-agent-bridge ping
chromium-agent-bridge listTabs
chromium-agent-bridge navigate '{"url":"https://google.com"}'
chromium-agent-bridge getContent '{"format":"text"}'
chromium-agent-bridge click '{"text":"Sign in"}'
chromium-agent-bridge type '{"selector":"input[name=q]","text":"tucson weather","submit":true}'
chromium-agent-bridge closeTab '{"tabId":123}'
```

Supported actions: `ping`, `listTabs`, `getActiveTab`, `setActiveTab`, `newSession`, `navigate`, `closeTab`, `screenshot`, `getContent`, `getInteractables`, `click`, `type`, `fillForm`, `waitFor`, `eval`, `scroll`.
