import { useEffect, useMemo, useRef, useState } from "react";

import type {
  Account,
  ProviderId,
  RemediationChoice,
  RemediationPlan,
} from "../types/quota";
import {
  executeRemediation,
  subscribeToClaudeLogin,
  subscribeToCodexLogin,
  type ProfileLoginEvent,
} from "../lib/ipc";
import { useModalDialog } from "../hooks/useModalDialog";
import { ProviderMark } from "./ProviderMark";

type FlowState =
  | { kind: "ready" }
  | { kind: "starting"; choiceId: string }
  | { kind: "browser"; profileId: string }
  | {
      kind: "terminal";
      command: string;
      provider: ProviderId;
      targetAccountId: string | null;
      initialActiveId: string | null;
    }
  | { kind: "success"; message: string }
  | { kind: "error"; message: string };

export interface RemediationDialogProps {
  account: Account;
  accounts: Account[];
  plan: RemediationPlan;
  onClose: () => void;
  onRefresh: () => void;
  onOpenSettings: (provider: ProviderId | null) => void;
}

const VERIFY_INTERVAL_MS = 3_000;
const VERIFY_TIMEOUT_MS = 5 * 60_000;

function activeAccountId(accounts: Account[], provider: ProviderId): string | null {
  return accounts.find((candidate) => candidate.provider === provider && candidate.isCliActive)?.id ?? null;
}

function buttonTone(choice: RemediationChoice, index: number): string {
  return choice.impact === "global-cli-identity" || index > 0
    ? "btn btn--ghost"
    : "btn btn--primary";
}

export function RemediationDialog({
  account,
  accounts,
  plan,
  onClose,
  onRefresh,
  onOpenSettings,
}: RemediationDialogProps) {
  const [flow, setFlow] = useState<FlowState>({ kind: "ready" });
  const [copied, setCopied] = useState(false);
  const dialogRef = useModalDialog(true, onClose);
  const pendingProfile = useRef<string | null>(null);
  const earlyLoginEvents = useRef(new Map<string, ProfileLoginEvent>());
  const accountsRef = useRef(accounts);
  accountsRef.current = accounts;

  useEffect(() => {
    let cancelled = false;
    const onLogin = (event: ProfileLoginEvent) => {
      if (!pendingProfile.current) {
        earlyLoginEvents.current.set(event.profileId, event);
        if (earlyLoginEvents.current.size > 16) {
          const oldest = earlyLoginEvents.current.keys().next().value;
          if (oldest) earlyLoginEvents.current.delete(oldest);
        }
        return;
      }
      if (event.profileId !== pendingProfile.current) return;
      pendingProfile.current = null;
      if (event.success) {
        setFlow({
          kind: "success",
          message: "Browser sign-in completed. EyeUrAI is refreshing accounts now; confirm the intended account appears live in the dashboard.",
        });
        onRefresh();
      } else {
        setFlow({
          kind: "error",
          message: event.message ?? "The browser sign-in was not completed.",
        });
      }
    };
    const subscriptions = Promise.all([
      subscribeToClaudeLogin(onLogin),
      subscribeToCodexLogin(onLogin),
    ]);
    return () => {
      cancelled = true;
      pendingProfile.current = null;
      earlyLoginEvents.current.clear();
      void subscriptions.then((unlisten) => {
        if (cancelled) unlisten.forEach((dispose) => dispose());
      });
    };
  }, [account.label, onRefresh]);

  useEffect(() => {
    if (flow.kind !== "terminal") return;
    const verify = () => onRefresh();
    const interval = window.setInterval(verify, VERIFY_INTERVAL_MS);
    const timeout = window.setTimeout(() => {
      setFlow({
        kind: "error",
        message: "EyeUrAI did not detect the account switch. You can recheck or copy the command and finish it manually.",
      });
    }, VERIFY_TIMEOUT_MS);
    return () => {
      window.clearInterval(interval);
      window.clearTimeout(timeout);
    };
  }, [flow.kind, onRefresh]);

  useEffect(() => {
    if (flow.kind !== "terminal") return;
    const target = flow.targetAccountId
      ? accounts.find((candidate) => candidate.id === flow.targetAccountId)
      : null;
    if (target?.isCliActive && target.status === "fresh") {
      setFlow({
        kind: "success",
        message: `${target.label} is now the active CLI account and its limits are live.`,
      });
      return;
    }
    const currentActive = activeAccountId(accounts, flow.provider);
    if (
      currentActive &&
      currentActive !== flow.initialActiveId &&
      currentActive !== flow.targetAccountId
    ) {
      const current = accounts.find((candidate) => candidate.id === currentActive);
      setFlow({
        kind: "error",
        message: `The CLI switched to ${current?.label ?? "a different account"}, not ${account.label}. Try the switch again and choose the intended account.`,
      });
    }
  }, [account.label, accounts, flow]);

  const terminalChoice = useMemo(
    () => plan.choices.find((choice) => choice.kind === "open-terminal"),
    [plan.choices],
  );

  async function copyCommand(command = terminalChoice?.commandPreview): Promise<void> {
    if (!command) return;
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      setFlow({ kind: "error", message: `Copy this command manually: ${command}` });
    }
  }

  async function run(choice: RemediationChoice): Promise<void> {
    setFlow({ kind: "starting", choiceId: choice.id });
    try {
      const result = await executeRemediation(plan.id, choice.id);
      if (!result) throw new Error("This action is available in the installed EyeUrAI app.");
      if (result.kind === "refresh_requested") {
        onRefresh();
        onClose();
        return;
      }
      if (result.kind === "open_settings") {
        onOpenSettings(result.openConnection ? result.provider : null);
        return;
      }
      if (result.kind === "login_started") {
        pendingProfile.current = result.profileId;
        const early = earlyLoginEvents.current.get(result.profileId);
        earlyLoginEvents.current.clear();
        if (early) {
          pendingProfile.current = null;
          if (early.success) {
            setFlow({
              kind: "success",
              message: "Browser sign-in completed. EyeUrAI is refreshing accounts now; confirm the intended account appears live in the dashboard.",
            });
            onRefresh();
          } else {
            setFlow({
              kind: "error",
              message: early.message ?? "The browser sign-in was not completed.",
            });
          }
        } else {
          setFlow({ kind: "browser", profileId: result.profileId });
        }
        return;
      }
      setFlow({
        kind: "terminal",
        command: result.command,
        provider: result.provider,
        targetAccountId: result.targetAccountId,
        initialActiveId: activeAccountId(accountsRef.current, result.provider),
      });
    } catch (cause) {
      setFlow({
        kind: "error",
        message: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }

  return (
    <div className="repair" role="dialog" aria-modal="true" aria-labelledby="repair-title" ref={dialogRef}>
      <div className="repair__card">
        <header className="repair__head">
          <span className="repair__mark"><ProviderMark provider={account.provider} size={20} /></span>
          <div>
            <h2 id="repair-title">{plan.title}</h2>
            <p>{account.label}</p>
          </div>
          <button type="button" className="repair__close" aria-label="Close repair" onClick={onClose}>×</button>
        </header>

        {flow.kind === "ready" || flow.kind === "starting" ? (
          <>
            <p className="repair__detail">{plan.detail}</p>
            <div className="repair__choices">
              {plan.choices.map((choice, index) => (
                <div className="repair__choice" data-impact={choice.impact} key={choice.id}>
                  <button
                    type="button"
                    className={buttonTone(choice, index)}
                    disabled={flow.kind === "starting"}
                    onClick={() => void run(choice)}
                  >
                    {flow.kind === "starting" && flow.choiceId === choice.id ? "Starting…" : choice.label}
                  </button>
                  {choice.detail ? <p>{choice.detail}</p> : null}
                  {choice.kind === "open-terminal" && choice.commandPreview ? (
                    <button type="button" className="repair__copy" onClick={() => void copyCommand(choice.commandPreview)}>
                      {copied ? "Copied" : `Copy ${choice.commandPreview}`}
                    </button>
                  ) : null}
                </div>
              ))}
            </div>
          </>
        ) : flow.kind === "browser" ? (
          <div className="repair__waiting" role="status">
            <span className="repair__spinner" aria-hidden="true" />
            <h3>Waiting for browser sign-in…</h3>
            <p>Finish choosing the intended account in your browser. EyeUrAI will refresh automatically.</p>
          </div>
        ) : flow.kind === "terminal" ? (
          <div className="repair__waiting" role="status">
            <span className="repair__spinner" aria-hidden="true" />
            <h3>Waiting for account switch…</h3>
            <p>Confirm the command in Terminal, complete the provider login, then return here.</p>
            <code>{flow.command}</code>
            <div className="repair__inlineActions">
              <button type="button" className="btn btn--ghost btn--mini" onClick={onRefresh}>Recheck</button>
              <button type="button" className="repair__copy" onClick={() => void copyCommand(flow.command)}>{copied ? "Copied" : "Copy command"}</button>
            </div>
          </div>
        ) : flow.kind === "success" ? (
          <div className="repair__result" data-tone="success" role="status">
            <h3>{flow.message.startsWith("Browser") ? "Sign-in complete" : "Connected"}</h3>
            <p>{flow.message}</p>
            <button type="button" className="btn btn--primary" onClick={onClose}>Done</button>
          </div>
        ) : (
          <div className="repair__result" data-tone="error" role="alert">
            <h3>Not fixed yet</h3>
            <p>{flow.message}</p>
            <div className="repair__inlineActions">
              <button type="button" className="btn btn--primary" onClick={() => setFlow({ kind: "ready" })}>Try again</button>
              {terminalChoice?.commandPreview ? <button type="button" className="repair__copy" onClick={() => void copyCommand()}>{copied ? "Copied" : "Copy command"}</button> : null}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
