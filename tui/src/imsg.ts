import {spawn, spawnSync} from 'node:child_process';
import {existsSync, readFileSync} from 'node:fs';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';

export type Role = 'mac' | 'linux' | 'unknown';

export interface PlatformInfo {
  os: string;
  arch: string;
  role: Role;
  hostname: string;
  config_paths: {
    bridge_config: string;
    sync_config: string;
    sync_state: string;
    plugin_dir: string;
  };
}

export interface DoctorCheck {
  name: string;
  ok: boolean;
  detail: string;
  scope?: string;
}

export interface DoctorReport {
  ok: boolean;
  checks: DoctorCheck[];
}

export interface JsonResult {
  ok: boolean;
  stdout: string;
  stderr: string;
  data: Record<string, unknown>;
  error: string;
}

function resolveImsgBinary(): string {
  const env = process.env.IMSG_BIN;
  if (env && existsSync(env)) return env;

  const home = process.env.HOME ?? '';
  const here = dirname(fileURLToPath(import.meta.url));
  const candidates = [
    join(here, '../../target/release/imsg'),
    join(here, '../../target/debug/imsg'),
    join(home, '.cargo/bin/imsg'),
    join(home, '.local/bin/imsg'),
    'imsg',
  ];
  for (const c of candidates) {
    if (existsSync(c)) return c;
  }
  return 'imsg';
}

const IMSG = resolveImsgBinary();

export function runImsg(args: string[], json = true): {ok: boolean; stdout: string; stderr: string} {
  const result = spawnSync(IMSG, [...(json ? ['--json'] : []), ...args], {
    encoding: 'utf8',
    env: process.env,
  });
  return {
    ok: (result.status ?? 1) === 0,
    stdout: result.stdout ?? '',
    stderr: result.stderr ?? '',
  };
}

function asJson(r: {ok: boolean; stdout: string; stderr: string}): JsonResult {
  try {
    const data = JSON.parse(r.stdout) as Record<string, unknown>;
    const err =
      typeof data.error === 'string'
        ? data.error
        : r.ok
          ? ''
          : lastLine(r.stderr || r.stdout);
    return {
      ok: r.ok && data.ok !== false,
      stdout: r.stdout,
      stderr: r.stderr,
      data,
      error: err,
    };
  } catch {
    return {
      ok: false,
      stdout: r.stdout,
      stderr: r.stderr,
      data: {},
      error: lastLine(r.stderr || r.stdout) || 'imsg returned no JSON',
    };
  }
}

export function lastLine(text: string): string {
  const line = text
    .trim()
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .at(-1);
  return (line ?? text.trim()).slice(0, 240);
}

export function getInfo(): PlatformInfo | null {
  const r = runImsg(['info']);
  if (!r.ok) return null;
  try {
    return JSON.parse(r.stdout) as PlatformInfo;
  } catch {
    return null;
  }
}

export function doctor(): DoctorReport | null {
  const r = runImsg(['doctor']);
  try {
    return JSON.parse(r.stdout) as DoctorReport;
  } catch {
    return {ok: false, checks: [{name: 'doctor', ok: false, detail: lastLine(r.stderr || r.stdout)}]};
  }
}

export function runImsgAsync(args: string[], json = true): Promise<{ok: boolean; stdout: string; stderr: string}> {
  return new Promise((resolve) => {
    const child = spawn(IMSG, [...(json ? ['--json'] : []), ...args], {env: process.env});
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    child.on('close', (code) => {
      resolve({ok: (code ?? 1) === 0, stdout, stderr});
    });
    child.on('error', (err) => {
      resolve({ok: false, stdout: '', stderr: err.message});
    });
  });
}

export async function setupStep(
  id: string,
  opts?: {bind?: string; mdns?: boolean},
): Promise<JsonResult> {
  const args = ['setup', 'step', id];
  if (opts?.bind) args.push('--bind', opts.bind);
  if (opts?.mdns) args.push('--mdns');
  return asJson(await runImsgAsync(args));
}

export async function setupPush(
  host: string,
  opts?: {code?: string; bind?: string; bin?: string; pluginRepo?: string},
): Promise<JsonResult> {
  const args = ['setup', 'push', '--ssh', host];
  if (opts?.code) args.push('--code', opts.code);
  if (opts?.bind) args.push('--bind', opts.bind);
  if (opts?.bin) args.push('--bin', opts.bin);
  if (opts?.pluginRepo) args.push('--plugin-repo', opts.pluginRepo);
  return asJson(await runImsgAsync(args));
}

export function setupPair(code: string, host: string) {
  return runImsg(['setup', 'pair', code, '--host', host, '--insecure']);
}

export function defaultSshHost(): string | null {
  return sshAliasConfigured('omarchy') ? 'omarchy' : null;
}

export function install(pluginPath?: string) {
  const args = ['install'];
  if (pluginPath) args.push('--plugin', pluginPath);
  return runImsg(args);
}

export function uninstall(purge = false) {
  return runImsg(['uninstall', ...(purge ? ['--purge'] : [])]);
}

export function tailscaleIp(): string | null {
  const r = spawnSync('tailscale', ['ip', '-4'], {encoding: 'utf8'});
  if (r.status !== 0) return null;
  const ip = (r.stdout ?? '').trim();
  return ip.startsWith('100.') ? ip : null;
}

export function sshAliasConfigured(alias: string): boolean {
  const r = spawnSync('ssh', ['-G', alias], {encoding: 'utf8'});
  if ((r.status ?? 1) !== 0) return false;
  const hostname = (r.stdout ?? '')
    .split('\n')
    .find((l) => l.startsWith('hostname '))
    ?.slice(9)
    .trim();
  if (hostname && hostname !== alias) return true;
  const config = join(process.env.HOME ?? '', '.ssh/config');
  if (!existsSync(config)) return false;
  const text = readFileSync(config, 'utf8');
  return new RegExp(`^Host\\s+([^\\n]*\\s)?${alias}(\\s|$)`, 'm').test(text);
}
