import type { MonitorAlert, PresentationStatus, Session, SessionStatus, Snapshot } from '../types/snapshot.js';

/** 展示状态文案；键覆盖 wire 状态与派生状态。 */
export const STATUS: Record<PresentationStatus, string> = { running: '运行中', wait: '待确认', done: '已完成', error: '失败', aborted: '已中止', unknown: '状态未知', idle: '空闲', offline: '已断连' };
export function sessionHeadline(session: Partial<Session> | null | undefined, status: PresentationStatus = 'idle'): string {
  const title=String(session?.title||'').trim();
  if(title)return title;
  const project=String(session?.project||'').trim();
  if(project && project!==session?.source)return project;
  return STATUS[status]||'空闲';
}
/** 仍在占位（未进入终态）的会话状态；终态的保留时长由 rail 自己控制。 */
export const ACTIVE: ReadonlySet<SessionStatus> = new Set<SessionStatus>(['running','wait']);
export const COMPLETION_HOLD_MS = 30 * 60_000;
const TERMINAL: ReadonlySet<SessionStatus> = new Set<SessionStatus>(['done','error','aborted']);

export interface AssignOptions {
  assignment?: string;
  seats?: string[];
  autoDiscover?: boolean;
  retentionHours?: number;
}

/**
 * 把会话分配到固定座位：保留仍活跃的旧座位，再按等待优先、更新时间排序补位。
 * 返回值与座位一一对应，空座为 null。
 */
export function assignSessions(previous: (string | null)[], sessions: Session[], now: number = Date.now(), count: number = 8, preferredSeats: Map<string, number> = new Map(), dismissed: Set<string> = new Set(), options: AssignOptions = {}): (string | null)[] {
  const allowed=(session: Session,i: number)=>options.assignment!=='fixed'||!options.seats?.[i]||options.seats[i]==='auto'||options.seats[i]===session.source;
  const byId = new Map<string, Session>(sessions.map((s): [string, Session] => [s.id,s]));
  const slots: (string | null)[] = Array.from({length:count},(_,i)=>previous[i] || null);
  const rowAt = (i: number): Session | undefined => { const id = slots[i]; return id ? byId.get(id) : undefined; };
  const candidates = sessions.filter(s=>ACTIVE.has(s.status)&&(options.autoDiscover!==false||previous.includes(s.id))).sort((a,b)=>Number(b.status==='wait')-Number(a.status==='wait')||b.updatedAt-a.updatedAt||a.id.localeCompare(b.id));
  for(let i=0;i<count;i++) {
    const current=rowAt(i);
    if(!current||!allowed(current,i))slots[i]=null;
    // No adapter emits idle; the guard is carried over from the office-era model
    // so this migration does not change behaviour.
    else if(String(current.status)==='idle')slots[i]=null;
    else if(dismissed.has(current.id) && TERMINAL.has(current.status))slots[i]=null;
    else if(TERMINAL.has(current.status) && now-(current.endedAt??current.updatedAt)>=(options.retentionHours??.5)*3600000)slots[i]=null;
  }
  const available=candidates.filter(s=>!slots.includes(s.id));
  // Only release a completed seat early when all empty seats will be needed.
  let needed=available.length-slots.filter(id=>!id).length;
  for(let i=0;i<count && needed>0;i++){const at=rowAt(i);if(at&&TERMINAL.has(at.status)){slots[i]=null;needed--;}}
  // Reuse a returning conversation's former resident if that resident is free.
  for(const session of available.slice(0,slots.filter(id=>!id).length)){const seat=preferredSeats.get(session.id);if(seat!==undefined&&Number.isInteger(seat)&&seat>=0&&seat<count&&!slots[seat]&&allowed(session,seat))slots[seat]=session.id;}
  const remaining=available.filter(s=>!slots.includes(s.id));
  for(let i=0;i<count;i++)if(!slots[i]){const index=remaining.findIndex(s=>allowed(s,i));if(index>=0)slots[i]=remaining.splice(index,1)[0].id;}
  return slots;
}

export interface NotificationTrackerOptions {
  storage?: {getItem(key: string): string | null | undefined; setItem(key: string, value: string): void} | null;
  now?: () => number;
}

export interface NotificationTracker {
  alerts: MonitorAlert[];
  ingest(snapshot: Snapshot): MonitorAlert[];
}

/** 跨刷新去重的通知跟踪器；只报告首次可见的 wait/终态事件。 */
export function createNotificationTracker(options: NotificationTrackerOptions = {}): NotificationTracker {
  const { storage, now = Date.now } = options;
  const key='astra.monitor.notifications.v1';
  let saved: {seen?: unknown[]; alerts?: MonitorAlert[]} | null = null;
  try { saved=JSON.parse(storage?.getItem(key)||'null') as {seen?: unknown[]; alerts?: MonitorAlert[]} | null; } catch {}
  const seen = new Set<unknown>(Array.isArray(saved?.seen)?saved.seen:[]);
  const alerts: MonitorAlert[] = Array.isArray(saved?.alerts)?saved.alerts.slice(0,60):[];
  let initialized=false,lastSaved=JSON.stringify({seen:[...seen].slice(-2000),alerts});
  const ingest = (snapshot: Snapshot): MonitorAlert[] => {
    if(!snapshot.ready)return [];
    const byId = new Map<string, Session>(snapshot.sessions.map((s): [string, Session] => [s.id,s]));
    const fresh: MonitorAlert[] = [];
    for(const event of snapshot.events) {
      if(seen.has(event.id))continue;
      seen.add(event.id);
      const session=byId.get(event.sessionId);
      const unresolved=event.kind==='wait' && session?.roundId===event.roundId && session.pending.some(p=>JSON.parse(event.id)[3]===p.id);
      if(event.kind==='wait' ? !unresolved : !initialized || event.historical)continue;
      const alert={...event, title:session?.title||event.title, receivedAt:now()}; fresh.push(alert); alerts.unshift(alert);
    }
    initialized=true;alerts.splice(60);
    while(seen.size>2000)seen.delete(seen.values().next().value);
    const serialized=JSON.stringify({seen:[...seen],alerts});
    if(serialized!==lastSaved)try { storage?.setItem(key,serialized);lastSaved=serialized; } catch {}
    return fresh;
  };
  return { alerts, ingest };
}
