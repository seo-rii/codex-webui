import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const checks = [
  {
    command: "cargo",
    args: ["fmt", "--check"],
    cwd: path.join(repoRoot, "backend")
  },
  {
    command: "cargo",
    args: ["test", "main_tests::auth_git_static::viewer_http_routes_match_websocket_authorization_policy"],
    cwd: path.join(repoRoot, "backend")
  },
  {
    command: "cargo",
    args: ["test", "main_tests::auth_git_static::health_readiness_and_metrics_endpoints_report_gateway_state"],
    cwd: path.join(repoRoot, "backend")
  },
  {
    command: "cargo",
    args: ["test", "main_tests::settings_and_automation::notification_webhook_resolution_skips_dns_pinning_for_literal_public_ip"],
    cwd: path.join(repoRoot, "backend")
  },
  {
    command: "cargo",
    args: ["test", "main_tests::settings_and_automation::forced_shutdown_requires_explicit_confirmation_phrase"],
    cwd: path.join(repoRoot, "backend")
  },
  {
    command: "cargo",
    args: ["test", "main_tests::settings_and_automation::corrupt_ui_state_recovers_from_previous_snapshot"],
    cwd: path.join(repoRoot, "backend")
  },
  {
    command: "cargo",
    args: ["test", "main_tests::attachments_and_recovery::attachment_cleanup_retains_recent_orphans_until_min_age"],
    cwd: path.join(repoRoot, "backend")
  },
  {
    command: "pnpm",
    args: ["verify:tunnel-safety"],
    cwd: repoRoot
  }
];

function runCheck({ command, args, cwd }) {
  return new Promise((resolve, reject) => {
    console.log(`\n$ ${command} ${args.join(" ")}`);
    const child = spawn(command, args, {
      cwd,
      env: process.env,
      stdio: "inherit"
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) {
        resolve();
        return;
      }
      reject(new Error(`${command} ${args.join(" ")} exited with ${code ?? "unknown"}`));
    });
  });
}

for (const check of checks) {
  await runCheck(check);
}
