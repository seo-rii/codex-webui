import { execFile } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

const AUTOSTART_LABEL = "dev.seorii.codex-webui";
const WINDOWS_STARTUP_SCRIPT = "codex-webui.vbs";
const MACOS_LAUNCH_AGENT = `${AUTOSTART_LABEL}.plist`;
const LINUX_SYSTEMD_SERVICE = "codex-webui-autostart.service";
const LINUX_DESKTOP_ENTRY = "codex-webui.desktop";

export type AutostartProvider =
  | "windows-startup"
  | "macos-launch-agent"
  | "linux-systemd-user"
  | "linux-xdg-autostart";

export type AutostartState = {
  available: boolean;
  enabled: boolean;
  provider: AutostartProvider | null;
  location: string | null;
};

type LaunchCommand = {
  packageRoot: string;
  nodeBinary: string;
  cliEntry: string;
};

function configHome() {
  const override = process.env.XDG_CONFIG_HOME?.trim();
  return override || path.join(os.homedir(), ".config");
}

function stateDir() {
  return path.join(os.homedir(), ".codex", "codex-webui");
}

function windowsStartupPath() {
  const appData = process.env.APPDATA?.trim();
  if (!appData) {
    return null;
  }
  return path.join(appData, "Microsoft", "Windows", "Start Menu", "Programs", "Startup", WINDOWS_STARTUP_SCRIPT);
}

function macosLaunchAgentPath() {
  return path.join(os.homedir(), "Library", "LaunchAgents", MACOS_LAUNCH_AGENT);
}

function linuxSystemdUserPath() {
  return path.join(configHome(), "systemd", "user", LINUX_SYSTEMD_SERVICE);
}

function linuxDesktopEntryPath() {
  return path.join(configHome(), "autostart", LINUX_DESKTOP_ENTRY);
}

async function pathExists(filePath: string | null) {
  if (!filePath) {
    return false;
  }
  try {
    await access(filePath);
    return true;
  } catch {
    return false;
  }
}

async function resolvePackageRoot() {
  const hints = [
    process.env.CODEX_WEBUI_PACKAGE_ROOT?.trim() || null,
    path.dirname(fileURLToPath(import.meta.url)),
    process.cwd()
  ].filter(Boolean) as string[];

  for (const hint of hints) {
    let current = path.resolve(hint);
    while (true) {
      const cliEntry = path.join(current, "bin", "codex-webui.mjs");
      if (await pathExists(cliEntry)) {
        return current;
      }
      const parent = path.dirname(current);
      if (parent === current) {
        break;
      }
      current = parent;
    }
  }

  throw new Error("Could not resolve the codex-webui package root.");
}

async function resolveLaunchCommand(): Promise<LaunchCommand> {
  const packageRoot = await resolvePackageRoot();
  const nodeBinary = process.execPath;
  const cliEntry = path.join(packageRoot, "bin", "codex-webui.mjs");
  if (!(await pathExists(nodeBinary))) {
    throw new Error(`Could not resolve the Node runtime at ${nodeBinary}.`);
  }
  if (!(await pathExists(cliEntry))) {
    throw new Error(`Could not resolve the codex-webui CLI entry at ${cliEntry}.`);
  }
  return {
    packageRoot,
    nodeBinary,
    cliEntry
  };
}

function escapeWindowsVbsString(value: string) {
  return value.replace(/"/gu, '""');
}

function escapeXml(value: string) {
  return value
    .replace(/&/gu, "&amp;")
    .replace(/</gu, "&lt;")
    .replace(/>/gu, "&gt;")
    .replace(/"/gu, "&quot;")
    .replace(/'/gu, "&apos;");
}

function escapeSystemdArg(value: string) {
  return `"${value.replace(/(["\\])/gu, "\\$1")}"`;
}

function escapeDesktopArg(value: string) {
  return `"${value.replace(/([\\"])/gu, "\\$1")}"`;
}

async function canUseLinuxSystemdUser() {
  try {
    await execFileAsync("systemctl", ["--user", "show-environment"]);
    return true;
  } catch {
    return false;
  }
}

async function preferredLinuxProvider(): Promise<AutostartProvider> {
  if (await pathExists(linuxSystemdUserPath())) {
    return "linux-systemd-user";
  }
  if (await pathExists(linuxDesktopEntryPath())) {
    return "linux-xdg-autostart";
  }
  return (await canUseLinuxSystemdUser()) ? "linux-systemd-user" : "linux-xdg-autostart";
}

async function writeWindowsStartupScript() {
  const targetPath = windowsStartupPath();
  if (!targetPath) {
    throw new Error("Windows startup folder is unavailable.");
  }
  const { nodeBinary, cliEntry } = await resolveLaunchCommand();
  await mkdir(path.dirname(targetPath), { recursive: true });
  await writeFile(
    targetPath,
    [
      'Set WshShell = CreateObject("WScript.Shell")',
      `WshShell.Run """" & "${escapeWindowsVbsString(nodeBinary)}" & """ """ & "${escapeWindowsVbsString(cliEntry)}" & """", 0, False`
    ].join("\r\n"),
    "utf8"
  );
  return targetPath;
}

async function writeMacosLaunchAgent() {
  const targetPath = macosLaunchAgentPath();
  const { packageRoot, nodeBinary, cliEntry } = await resolveLaunchCommand();
  const logFilePath = path.join(stateDir(), "autostart-launch.log");
  await mkdir(path.dirname(targetPath), { recursive: true });
  await mkdir(path.dirname(logFilePath), { recursive: true });
  await writeFile(
    targetPath,
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>${AUTOSTART_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
      <string>${escapeXml(nodeBinary)}</string>
      <string>${escapeXml(cliEntry)}</string>
    </array>
    <key>WorkingDirectory</key>
    <string>${escapeXml(packageRoot)}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>${escapeXml(logFilePath)}</string>
    <key>StandardErrorPath</key>
    <string>${escapeXml(logFilePath)}</string>
  </dict>
</plist>
`,
    "utf8"
  );

  if (typeof process.getuid === "function") {
    const domain = `gui/${process.getuid()}`;
    await execFileAsync("launchctl", ["bootout", domain, targetPath]).catch(() => null);
    await execFileAsync("launchctl", ["bootstrap", domain, targetPath]).catch(() => null);
  }

  return targetPath;
}

async function writeLinuxSystemdUserService() {
  const targetPath = linuxSystemdUserPath();
  const { packageRoot, nodeBinary, cliEntry } = await resolveLaunchCommand();
  await mkdir(path.dirname(targetPath), { recursive: true });
  await writeFile(
    targetPath,
    `[Unit]
Description=Codex Web UI autostart

[Service]
Type=oneshot
WorkingDirectory=${escapeSystemdArg(packageRoot)}
ExecStart=${escapeSystemdArg(nodeBinary)} ${escapeSystemdArg(cliEntry)}
RemainAfterExit=yes

[Install]
WantedBy=default.target
`,
    "utf8"
  );

  await execFileAsync("systemctl", ["--user", "daemon-reload"]);
  await execFileAsync("systemctl", ["--user", "enable", LINUX_SYSTEMD_SERVICE]);
  return targetPath;
}

async function writeLinuxDesktopEntry() {
  const targetPath = linuxDesktopEntryPath();
  const { packageRoot, nodeBinary, cliEntry } = await resolveLaunchCommand();
  await mkdir(path.dirname(targetPath), { recursive: true });
  await writeFile(
    targetPath,
    `[Desktop Entry]
Type=Application
Version=1.0
Name=Codex Web UI
Comment=Start Codex Web UI automatically when you sign in
Exec=${escapeDesktopArg(nodeBinary)} ${escapeDesktopArg(cliEntry)}
Path=${packageRoot}
Terminal=false
X-GNOME-Autostart-enabled=true
Hidden=false
`,
    "utf8"
  );
  return targetPath;
}

async function disableWindowsStartup() {
  const targetPath = windowsStartupPath();
  if (!targetPath) {
    return;
  }
  await rm(targetPath, { force: true });
}

async function disableMacosLaunchAgent() {
  const targetPath = macosLaunchAgentPath();
  if (typeof process.getuid === "function") {
    await execFileAsync("launchctl", ["bootout", `gui/${process.getuid()}`, targetPath]).catch(() => null);
  }
  await rm(targetPath, { force: true });
}

async function disableLinuxAutostart() {
  const systemdPath = linuxSystemdUserPath();
  const desktopPath = linuxDesktopEntryPath();
  if (await pathExists(systemdPath)) {
    await execFileAsync("systemctl", ["--user", "disable", LINUX_SYSTEMD_SERVICE]).catch(() => null);
    await rm(systemdPath, { force: true });
    await execFileAsync("systemctl", ["--user", "daemon-reload"]).catch(() => null);
  }
  await rm(desktopPath, { force: true });
}

export async function getAutostartState(): Promise<AutostartState> {
  try {
    await resolveLaunchCommand();
  } catch {
    return {
      available: false,
      enabled: false,
      provider: null,
      location: null
    };
  }

  if (process.platform === "win32") {
    const location = windowsStartupPath();
    return {
      available: Boolean(location),
      enabled: await pathExists(location),
      provider: location ? "windows-startup" : null,
      location
    };
  }

  if (process.platform === "darwin") {
    const location = macosLaunchAgentPath();
    return {
      available: true,
      enabled: await pathExists(location),
      provider: "macos-launch-agent",
      location
    };
  }

  if (process.platform === "linux") {
    const provider = await preferredLinuxProvider();
    const location = provider === "linux-systemd-user" ? linuxSystemdUserPath() : linuxDesktopEntryPath();
    return {
      available: true,
      enabled: await pathExists(location),
      provider,
      location
    };
  }

  return {
    available: false,
    enabled: false,
    provider: null,
    location: null
  };
}

export async function saveAutostartEnabled(enabled: boolean): Promise<AutostartState> {
  if (!enabled) {
    if (process.platform === "win32") {
      await disableWindowsStartup();
    } else if (process.platform === "darwin") {
      await disableMacosLaunchAgent();
    } else if (process.platform === "linux") {
      await disableLinuxAutostart();
    }
    return getAutostartState();
  }

  if (process.platform === "win32") {
    await writeWindowsStartupScript();
    return getAutostartState();
  }

  if (process.platform === "darwin") {
    await writeMacosLaunchAgent();
    return getAutostartState();
  }

  if (process.platform === "linux") {
    if (await canUseLinuxSystemdUser()) {
      try {
        await writeLinuxSystemdUserService();
        return getAutostartState();
      } catch {
        await rm(linuxSystemdUserPath(), { force: true }).catch(() => null);
      }
    }

    await writeLinuxDesktopEntry();
    return getAutostartState();
  }

  throw new Error(`Automatic startup is not supported on ${process.platform}.`);
}
