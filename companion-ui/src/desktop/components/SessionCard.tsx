import { pendingKind, permissionToolLabel, sessionPresentation } from '../../monitor/presentation.js';
import { prolongedPermissionCheck } from '../../monitor/permission-check.js';
import type { ConnectionState } from '../../types/snapshot.js';
import type { RailItem } from '../rail-model.js';
import type { RailController } from '../rail-controller.js';
import { railProjectLabel } from '../project-label.js';
import { ProviderIcons, providerLabel } from './provider.js';

const DISMISSABLE = new Set(['wait', 'done', 'error', 'aborted']);

/** Shield outline: a permission gate, as opposed to a question mark for a prompt. */
function PermissionShield() {
  return (
    <svg className="desktop-card-shield" viewBox="0 0 12 12" fill="none" aria-hidden="true" focusable="false">
      <path d="M6 1.2 2.4 2.7v3c0 2.2 1.5 3.9 3.6 4.8 2.1-.9 3.6-2.6 3.6-4.8v-3L6 1.2Z" stroke="currentColor" strokeWidth="1.1" strokeLinejoin="round" />
      <path d="M6 4.1v2" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
      <circle cx="6" cy="7.7" r="0.55" fill="currentColor" />
    </svg>
  );
}

/**
 * The body shared by the hover card and the automatic question card. The two
 * differ only in which connection state they are told about, which the caller
 * resolves — the automatic card ignores a session's own offline flag, exactly as
 * before.
 */
export function SessionCard({item, connection, surfaceKey, controller}: {
  item: RailItem;
  connection: ConnectionState;
  surfaceKey: string;
  controller: RailController;
}) {
  const presentation = sessionPresentation(item.session, connection);
  const text = presentation.question || presentation.title;
  const label = providerLabel(item, presentation);
  // A badge may carry a longer explanation (a delegated child names its parent).
  const providerHint = presentation.badge?.detail ?? label;
  const project = railProjectLabel(item.session, presentation.provider);
  const slowPermission = prolongedPermissionCheck(item.session);
  // A permission gate is a different errand for the user than a question, so the
  // card says which one it is and shows the tool that is being held.
  const permission = item.session.status === 'wait' && pendingKind(item.session.pending?.[0]) === 'permission';
  const kindLabel = permission ? `需要授权：${permissionToolLabel(item.session.pending?.[0]?.tool)}` : undefined;
  const dismissTitle = item.session.status === 'wait' ? (permission ? '关闭本次授权提示' : '关闭本次待确认提示')
    : slowPermission ? '关闭本次权限提醒'
      : item.session.status === 'error' ? '关闭本次失败提示'
        : '收起已完成任务';
  return (
    <div className="desktop-card-body" data-kind={permission ? 'permission' : 'question'}>
      <div className="desktop-card-head">
        <strong title={project}>{project}</strong>
        <span className="desktop-card-status" data-status={presentation.status} title={kindLabel}>
          {permission && <PermissionShield />}
          {presentation.statusLabel}
        </span>
        <span className="desktop-provider" title={providerHint} aria-label={providerHint}>
          <ProviderIcons item={item} presentation={presentation} />
        </span>
      </div>
      <button
        type="button"
        className="desktop-preview"
        disabled={!presentation.url}
        aria-label={presentation.url ? `${presentation.action}：${presentation.question || presentation.title}` : undefined}
        onClick={() => controller.clickAvatar(item.id)}
        ref={element => controller.hitRegions.register(`${surfaceKey}:preview`, 'control', element)}
      >
        <span className="desktop-preview-text" title={text}>{text}</span>
        {presentation.url && (
          <svg className="desktop-chevron" viewBox="0 0 16 16" fill="none" aria-hidden="true" focusable="false">
            <path d="m6 3.5 4.5 4.5L6 12.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        )}
      </button>
      {(DISMISSABLE.has(item.session.status) || slowPermission) && (
        <button
          type="button"
          className="desktop-dismiss"
          title={dismissTitle}
          aria-label={dismissTitle}
          onClick={() => controller.dismiss(item.id)}
          ref={element => controller.hitRegions.register(`${surfaceKey}:dismiss`, 'control', element)}
        />
      )}
    </div>
  );
}
