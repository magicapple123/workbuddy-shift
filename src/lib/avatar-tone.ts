/**
 * 头像身份色：6 组低饱和色，定义在 `src/index.css` 的 `--avatar-N-bg/-fg`
 * （深色主题在 `.dark` 覆盖），这里只返回工具类名。
 *
 * **不要**改回 Tailwind 调色板字面量（emerald/amber/sky 那套 100/800 的类名）：
 * 那套色会把注意力从主色抢走，而且深色主题下无法成对降饱和。
 */
const AVATAR_TONES = [
  "avatar-tone-0",
  "avatar-tone-1",
  "avatar-tone-2",
  "avatar-tone-3",
  "avatar-tone-4",
  "avatar-tone-5",
] as const;

export type AvatarTone = (typeof AVATAR_TONES)[number];

/** 按名字哈希取固定色调：同一账号在卡片、弹窗等位置的头像颜色一致。 */
export function avatarTone(name: string): AvatarTone {
  let hash = 0;
  for (let i = 0; i < name.length; i += 1) hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  return AVATAR_TONES[hash % AVATAR_TONES.length];
}
