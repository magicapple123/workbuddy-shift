import type { SourceId } from './settings.js';

export interface IntegrationStatus {
  source: SourceId;
  /** `hooks`: 装 hook；`webhook`: 注册回调；`log`: 只读轮询本地日志（DSH）。 */
  kind: 'hooks' | 'webhook' | 'log';
  status: 'installed' | 'not_installed' | 'partial' | 'error' | 'unavailable' | 'pending';
  message: string;
  locations: string[];
  automatic: boolean;
  lastEventAt?: number | null;
}
export interface Integrations { sources: IntegrationStatus[] }
export interface IntegrationAction { source: SourceId; action: 'install' | 'uninstall' }
