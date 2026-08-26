import React, {useEffect, useRef, useState} from 'react';
import {Box, Text, useApp, useInput} from 'ink';
import SelectInput from 'ink-select-input';
import TextInput from 'ink-text-input';
import Spinner from 'ink-spinner';
import {
  doctor,
  getInfo,
  lastLine,
  setupPush,
  setupStep,
  sshAliasConfigured,
  tailscaleIp,
  uninstall,
  type PlatformInfo,
} from './imsg.js';

export type StepId =
  | 'detect'
  | 'brew'
  | 'certs'
  | 'service'
  | 'enroll'
  | 'ssh-bin'
  | 'ssh-pair'
  | 'ssh-service'
  | 'ssh-plugin';

export type StepState = 'pending' | 'running' | 'ok' | 'fail' | 'skipped';
export type Step = {id: StepId; title: string; state: StepState; detail?: string};
export type PairingCode = {value: string; expiresAt?: string};

export type Screen =
  | {kind: 'boot'}
  | {kind: 'run'; steps: Step[]; code?: PairingCode}
  | {kind: 'ask-ssh'; steps: Step[]; code: PairingCode; bind: string}
  | {kind: 'ssh-form'; host: string; code: {value: string}}
  | {kind: 'ssh-run'; host: string; steps: Step[]; code: {value: string}}
  | {kind: 'ready'; summary: string; code?: string}
  | {kind: 'doctor'}
  | {kind: 'uninstall'; purge: boolean}
  | {kind: 'fail'; at: StepId; error: string; steps: Step[]};

const MAC_STEPS: Step[] = [
  {id: 'detect', title: 'Detected this Mac', state: 'pending'},
  {id: 'brew', title: 'Homebrew imsg', state: 'pending'},
  {id: 'certs', title: 'TLS certificates', state: 'pending'},
  {id: 'service', title: 'LaunchAgent installed', state: 'pending'},
  {id: 'enroll', title: 'Enroll listening', state: 'pending'},
];

const SSH_STEPS: Step[] = [
  {id: 'ssh-bin', title: 'Linux imsg installed', state: 'pending'},
  {id: 'ssh-pair', title: 'Paired over SSH', state: 'pending'},
  {id: 'ssh-service', title: 'Linux service enabled', state: 'pending'},
  {id: 'ssh-plugin', title: 'Omarchy plugin', state: 'pending'},
];

function mark(steps: Step[], id: StepId, state: StepState, detail?: string): Step[] {
  return steps.map((s) => (s.id === id ? {...s, state, detail} : s));
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v.length > 0 ? v : undefined;
}

function timeLeft(expiresAt?: string, now = Date.now()): string {
  if (!expiresAt) return '';
  const ms = new Date(expiresAt).getTime() - now;
  if (Number.isNaN(ms) || ms <= 0) return 'expired';
  const m = Math.floor(ms / 60000);
  const s = Math.floor((ms % 60000) / 1000);
  return `${m}:${s.toString().padStart(2, '0')} left`;
}

function Checklist({steps}: {steps: Step[]}) {
  return (
    <Box flexDirection="column" marginTop={1}>
      {steps.map((step) => (
        <Box key={step.id}>
          {step.state === 'running' ? (
            <Text color="cyan">
              <Spinner type="dots" />
            </Text>
          ) : (
            <Text
              color={
                step.state === 'ok'
                  ? 'green'
                  : step.state === 'fail'
                    ? 'red'
                    : step.state === 'skipped'
                      ? 'yellow'
                      : 'gray'
              }
            >
              {step.state === 'ok' ? '✓' : step.state === 'fail' ? '✗' : step.state === 'skipped' ? '–' : '·'}
            </Text>
          )}
          <Text> {step.title}</Text>
          {step.detail && step.state !== 'running' && (
            <Text dimColor>  {step.detail}</Text>
          )}
        </Box>
      ))}
    </Box>
  );
}

function CodeBanner({code}: {code?: PairingCode | {value: string; expiresAt?: string}}) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  if (!code?.value) return null;
  const left = timeLeft(code.expiresAt, now);
  return (
    <Box marginTop={1}>
      <Text bold color="yellow">
        Pairing code  {code.value}
      </Text>
      {left ? <Text color="yellow">   {left}</Text> : null}
    </Box>
  );
}

function Footer({hint}: {hint?: string}) {
  return (
    <Text dimColor>
      {hint ? `${hint} · ` : ''}d doctor · u uninstall · q quit
    </Text>
  );
}

export function App() {
  const {exit} = useApp();
  const [screen, setScreen] = useState<Screen>({kind: 'boot'});
  const [info, setInfo] = useState<PlatformInfo | null>(null);
  const [bind, setBind] = useState('');
  const [hostDraft, setHostDraft] = useState('omarchy');
  const back = useRef<Screen | null>(null);
  const omarchyAlias = useRef(false);
  const lastSsh = useRef<{host: string; code: string}>({host: 'omarchy', code: ''});

  useEffect(() => {
    const platform = getInfo();
    setInfo(platform);
    const ts = tailscaleIp();
    if (ts) setBind(ts);
    omarchyAlias.current = sshAliasConfigured('omarchy');
    if (platform?.role !== 'mac') {
      setScreen({
        kind: 'ready',
        summary: 'Run imsg setup on the Mac. This machine is paired from there over SSH.',
      });
      return;
    }
    void runMac(ts ?? undefined);
  }, []);

  useInput((input, key) => {
    if (screen.kind === 'ssh-form') return;
    if (input === 'q' && (screen.kind === 'ready' || screen.kind === 'ask-ssh' || screen.kind === 'boot')) {
      exit();
    }
    if (input === 'd' && screen.kind !== 'doctor') {
      back.current = screen;
      setScreen({kind: 'doctor'});
    }
    if (input === 'u' && screen.kind !== 'uninstall') {
      back.current = screen;
      setScreen({kind: 'uninstall', purge: false});
    }
    if (key.escape && (screen.kind === 'doctor' || screen.kind === 'uninstall') && back.current) {
      setScreen(back.current);
    }
  });

  async function runMac(fromBind?: string, fromId?: StepId) {
    let steps = MAC_STEPS.map((s) => ({...s}));
    let code: PairingCode | undefined;
    let nextBind = fromBind ?? bind;
    setScreen({kind: 'run', steps, code});

    const start = fromId ? steps.findIndex((s) => s.id === fromId) : 0;
    if (start > 0) {
      steps = steps.map((s, i) => (i < start ? {...s, state: 'ok' as StepState} : s));
    }
    const ids = steps.map((s) => s.id).slice(Math.max(start, 0));

    for (const id of ids) {
      steps = mark(steps, id, 'running');
      setScreen({kind: 'run', steps, code});
      const r = await setupStep(id, nextBind ? {bind: nextBind} : undefined);
      if (!r.ok) {
        const error = lastLine(r.error || r.stderr || r.stdout);
        steps = mark(steps, id, 'fail', error);
        setScreen({kind: 'fail', at: id, error, steps});
        return;
      }
      const detected = str(r.data.bind);
      if (detected) {
        nextBind = detected;
        setBind(detected);
      }
      const pairing = str(r.data.pairing_code);
      if (pairing) {
        code = {value: pairing, expiresAt: str(r.data.expires_at)};
      }
      steps = mark(steps, id, 'ok', str(r.data.detail));
      setScreen({kind: 'run', steps, code});
    }

    if (!code?.value) {
      steps = mark(steps, 'certs', 'fail', 'no pairing code');
      setScreen({kind: 'fail', at: 'certs', error: 'no pairing code after enroll', steps});
      return;
    }
    setScreen({kind: 'ask-ssh', steps, code, bind: nextBind});
  }

  async function runSsh(host: string, code: {value: string}) {
    lastSsh.current = {host, code: code.value};
    let steps = SSH_STEPS.map((s) => ({...s, state: 'running' as StepState}));
    setScreen({kind: 'ssh-run', host, steps, code});
    const r = await setupPush(host, {code: code.value, bind});
    if (!r.ok) {
      const error = lastLine(r.error || r.stderr || r.stdout);
      const raw = r.data.steps;
      if (Array.isArray(raw)) {
        steps = steps.map((s) => {
          const hit = raw.find((row) => {
            return Boolean(row && typeof row === 'object' && 'id' in row && (row as {id: string}).id === s.id);
          }) as {ok?: boolean; detail?: string} | undefined;
          if (!hit) return {...s, state: 'fail' as StepState, detail: error};
          return {
            ...s,
            state: hit.ok === false ? 'fail' : 'ok',
            detail: hit.detail,
          } as Step;
        });
      } else {
        steps = mark(steps, 'ssh-bin', 'fail', error);
      }
      const at = steps.find((s) => s.state === 'fail')?.id ?? 'ssh-bin';
      setScreen({kind: 'fail', at, error, steps});
      return;
    }
    const raw = r.data.steps;
    if (Array.isArray(raw)) {
      steps = steps.map((s) => {
        const hit = raw.find((row) => {
          return Boolean(row && typeof row === 'object' && 'id' in row && (row as {id: string}).id === s.id);
        }) as {ok?: boolean; detail?: string} | undefined;
        if (!hit) return {...s, state: 'ok' as StepState};
        return {
          ...s,
          state: hit.ok === false ? 'fail' : 'ok',
          detail: hit.detail,
        } as Step;
      });
    } else {
      steps = steps.map((s) => ({...s, state: 'ok' as StepState}));
    }
    setScreen({
      kind: 'ready',
      summary: `Omarchy host ${host} is paired.`,
      code: code.value,
    });
  }

  const headerBind = bind || tailscaleIp();

  return (
    <Box flexDirection="column" padding={1}>
      <Text bold color="cyan">
        iMessage for Omarchy
      </Text>
      <Text dimColor>
        {info ? `This Mac · ${info.os}/${info.arch}` : 'starting'}
        {headerBind ? ` · tailscale ${headerBind}` : ''}
      </Text>

      {screen.kind === 'boot' && (
        <Box marginTop={1}>
          <Spinner type="dots" />
          <Text> Detecting…</Text>
        </Box>
      )}

      {screen.kind === 'run' && (
        <>
          <Checklist steps={screen.steps} />
          <CodeBanner code={screen.code} />
          <Box marginTop={1}>
            <Footer />
          </Box>
        </>
      )}

      {screen.kind === 'ask-ssh' && (
        <>
          <Checklist steps={screen.steps} />
          <CodeBanner code={screen.code} />
          <Box flexDirection="column" marginTop={1}>
            <Text bold>Connect Omarchy over SSH?</Text>
            <SelectInput
              items={[
                ...(omarchyAlias.current
                  ? [{label: 'Yes, use host omarchy', value: 'omarchy'}]
                  : []),
                {label: 'Enter a different SSH host', value: 'form'},
                {label: 'Skip, pair from Linux later', value: 'skip'},
              ]}
              onSelect={(item) => {
                if (item.value === 'skip') {
                  setScreen({
                    kind: 'ready',
                    summary: 'Mac is ready. Pair from Linux later if you skipped SSH.',
                    code: screen.code.value,
                  });
                  return;
                }
                if (item.value === 'form') {
                  setHostDraft(omarchyAlias.current ? 'omarchy' : '');
                  setScreen({kind: 'ssh-form', host: hostDraft, code: {value: screen.code.value}});
                  return;
                }
                void runSsh('omarchy', {value: screen.code.value});
              }}
            />
          </Box>
          <Footer />
        </>
      )}

      {screen.kind === 'ssh-form' && (
        <Box flexDirection="column" marginTop={1}>
          <Text>SSH host (user@name or an ssh config alias):</Text>
          <TextInput
            value={hostDraft}
            onChange={setHostDraft}
            onSubmit={(value) => {
              const host = value.trim();
              if (!host) return;
              void runSsh(host, screen.code);
            }}
          />
          <Text dimColor>Enter to push · Esc is ignored while typing</Text>
        </Box>
      )}

      {screen.kind === 'ssh-run' && (
        <>
          <Text>SSH {screen.host}</Text>
          <Checklist steps={screen.steps} />
          <CodeBanner code={screen.code} />
        </>
      )}

      {screen.kind === 'ready' && (
        <Box flexDirection="column" marginTop={1}>
          <Text color="green">{screen.summary}</Text>
          {screen.code && (
            <Text bold color="yellow">
              Pairing code  {screen.code}
            </Text>
          )}
          <Box marginTop={1}>
            <Footer hint="done" />
          </Box>
        </Box>
      )}

      {screen.kind === 'fail' && (
        <Box flexDirection="column" marginTop={1}>
          <Checklist steps={screen.steps} />
          <Text color="red">{screen.error}</Text>
          <SelectInput
            items={[
              {label: 'Retry this step', value: 'retry'},
              {label: 'Doctor', value: 'doctor'},
            ]}
            onSelect={(item) => {
              if (item.value === 'doctor') {
                back.current = screen;
                setScreen({kind: 'doctor'});
                return;
              }
              if (screen.at.startsWith('ssh-')) {
                void runSsh(lastSsh.current.host, {value: lastSsh.current.code});
                return;
              }
              void runMac(bind || undefined, screen.at);
            }}
          />
        </Box>
      )}

      {screen.kind === 'doctor' && <DoctorView />}

      {screen.kind === 'uninstall' && (
        <Box flexDirection="column" marginTop={1}>
          <SelectInput
            items={[
              {label: 'Uninstall service only', value: 'soft'},
              {label: 'Uninstall + purge certs/data', value: 'purge'},
              {label: 'Cancel', value: 'cancel'},
            ]}
            onSelect={(item) => {
              if (item.value === 'cancel') {
                if (back.current) setScreen(back.current);
                return;
              }
              const purge = item.value === 'purge';
              const r = uninstall(purge);
              if (r.ok) {
                setScreen({
                  kind: 'ready',
                  summary: purge ? 'Uninstalled and purged.' : 'Uninstalled.',
                });
                return;
              }
              setScreen({
                kind: 'fail',
                at: 'detect',
                error: lastLine(r.stderr || r.stdout),
                steps: MAC_STEPS.map((s) => ({...s})),
              });
            }}
          />
        </Box>
      )}
    </Box>
  );
}

function DoctorView() {
  const [report] = useState(doctor());
  return (
    <Box flexDirection="column" marginTop={1}>
      <Text bold>Doctor</Text>
      {report?.checks.map((c) => (
        <Text key={`${c.scope ?? ''}-${c.name}`} color={c.ok ? 'green' : 'red'}>
          {c.ok ? '✓' : '✗'} {c.scope ? `[${c.scope}] ` : ''}
          {c.name}: {c.detail}
        </Text>
      ))}
      <Text dimColor>Esc to return</Text>
    </Box>
  );
}
