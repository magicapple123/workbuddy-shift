import internalPrompts from './codex-internal-prompts.json' with { type: 'json' };
import type { Session } from '../types/snapshot.js';

// Narrow, observed templates only. Do not hide user tasks based on keywords,
// cwd, missing titles, or the mere presence of a background process.
export function isInternalCodexPrompt(prompt: unknown): boolean {
  const normalized = String(prompt || '').trim().replace(/\s+/g, ' ');
  return internalPrompts.some(prefix => normalized.startsWith(prefix));
}
/** Codex 的内部提示词会话不进悬浮窗；其他来源一律可见。 */
export function visibleSession(session: Session): boolean {
  return session.source !== 'codex' || !isInternalCodexPrompt(session.title);
}
