/**
 * 状态 chip 的公共尺寸 / 字号（卡片状态区与积分区的徽标共用）。
 *
 * 单独成模块是为了避免 `status-chips.tsx` ↔ `overflow-chip.tsx` 的循环依赖
 * （`+N` 徽标也要用同一套尺寸）。改动会同时影响状态区与积分区徽标，改前先看两处。
 */
export const chipClass = "rounded-md px-1.5 py-0 text-[11px] font-medium";
