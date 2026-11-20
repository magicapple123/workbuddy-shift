/**
 * Rail-side attention for a tool permission gate (`R5-09`).
 *
 * Two things are deliberately *not* here:
 *
 * 1. Any allow/deny decision. The rail reports that the client is blocking and
 *    offers a jump to it; the choice stays in WorkBuddy's own dialog.
 * 2. Any OS notification. That path already exists end to end and lives in the
 *    runtime/desktop shell (`companion-runtime` collects `notifications` for
 *    `wait` events and `companion-desktop` shows them), and the rail window's
 *    capability set does not include `notification:allow-notify`. Re-sending
 *    from the WebView would duplicate the toast or throw. What the rail owes the
 *    user when no toast appears is a visible highlight — that is this module.
 */
import { isDesktop, onDesktopWindowActive } from '../desktop/host.js';
import { pendingKind } from './presentation.js';
import type { Subscribe } from './transport.js';
import type { Snapshot } from '../types/snapshot.js';

/** How long the fallback highlight stays on the rail. */
export const PERMISSION_ATTENTION_MS = 2_400;

const timers = new WeakMap<HTMLElement, ReturnType<typeof setTimeout>>();

/**
 * Flash the rail once. Kept as a plain DOM call so it can be invoked from a
 * snapshot callback without a React render: the rail controller owns the
 * container's other classes, and this one is additive and self-removing.
 */
export function flashRailAttention(element: HTMLElement | null): void {
  if (!element) return;
  const previous = timers.get(element);
  if (previous !== undefined) clearTimeout(previous);
  element.classList.add('permission-attention');
  timers.set(element, setTimeout(() => {
    element.classList.remove('permission-attention');
    timers.delete(element);
  }, PERMISSION_ATTENTION_MS));
}

/** `sessionId:pendingId` for every permission gate the snapshot currently shows. */
export function permissionGateKeys(snapshot: Snapshot): Set<string> {
  const keys = new Set<string>();
  for (const session of snapshot.sessions) {
    if (session.status !== 'wait') continue;
    for (const pending of session.pending || []) {
      if (pendingKind(pending) !== 'permission') continue;
      keys.add(`${session.id}:${String(pending.id)}`);
    }
  }
  return keys;
}

export interface PermissionAttentionOptions {
  /** The rail container that receives the fallback highlight. */
  container?: HTMLElement | null;
  /** Replaceable for tests and for the demo build. */
  flash?: (element: HTMLElement | null) => void;
  /** Initial window-focus guess; the native event refines it. */
  focused?: () => boolean;
}

export interface PermissionAttention {
  subscribe: Subscribe;
  dispose(): void;
}

function initialFocus(): boolean {
  try { return typeof document !== 'undefined' && document.hasFocus(); } catch { return false; }
}

/**
 * Wrap a snapshot subscription so a permission gate that becomes visible while
 * the rail window is in the background flashes the rail once.
 *
 * The gate is only visible after `hub.rs` released its 2.5s grace window, and a
 * gate that the client auto-approved was resolved before that, so this never
 * fires for an auto-approved tool. `seen` is pruned against the current
 * snapshot, so a vanished gate does not pin its key forever.
 */
export function withPermissionAttention(subscribe: Subscribe, options: PermissionAttentionOptions = {}): PermissionAttention {
  const { container = null, flash = flashRailAttention, focused = initialFocus } = options;
  const seen = new Set<string>();
  let active = focused();
  const stop = isDesktop() ? onDesktopWindowActive(value => { active = value; }) : () => {};
  const ingest = (snapshot: Snapshot) => {
    if (!snapshot.ready) return;
    const live = permissionGateKeys(snapshot);
    for (const key of live) {
      if (seen.has(key)) continue;
      seen.add(key);
      if (!active && container) flash(container);
    }
    for (const key of seen) if (!live.has(key)) seen.delete(key);
    while (seen.size > 256) {
      const oldest = seen.values().next().value;
      if (oldest === undefined) break;
      seen.delete(oldest);
    }
  };
  return {
    subscribe: (onSnapshot, onConnection) => subscribe(
      snapshot => { ingest(snapshot); onSnapshot(snapshot); },
      onConnection,
    ),
    dispose() { stop(); },
  };
}
