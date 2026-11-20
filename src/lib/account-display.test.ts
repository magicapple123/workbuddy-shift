import { describe, expect, it } from "vitest";

import { accountIdentity, displayName } from "./account-display";

/**
 * R5-07：显示名七级回退。
 *
 * 用例与后端 `crates/wb-shift-core/src/modules/account/tests/display.rs` 的
 * `display_name_*` 一一对应——两边规则必须逐条一致，改一处必须同步另一处。
 */
describe("account-display", () => {
  const base = { nickname: "林小凤", uid: "u1", id: "i1", email: "a@b.com" };

  it("有备注时显示备注（备注优先于昵称 / 邮箱）", () => {
    expect(displayName({ ...base, note: "设计组-主号" })).toBe("设计组-主号");
    // 存量数据里被钉在昵称上的账号也跟着备注走：displayField === "nickname" 不再压制备注。
    expect(displayName({ ...base, note: "设计组-主号", displayField: "nickname" })).toBe(
      "设计组-主号",
    );
    // `note` 偏好与新默认等价，保留只为兼容存量数据。
    expect(displayName({ ...base, note: "设计组-主号", displayField: "note" })).toBe("设计组-主号");
  });

  it("显式选手机号时手机号压过备注（唯一例外）", () => {
    expect(
      displayName({ ...base, note: "设计组-主号", phoneNumber: "13800000000", displayField: "phone" }),
    ).toBe("13800000000");
    // 没有手机号时继续往下走，而不是显示空。
    expect(displayName({ ...base, note: "设计组-主号", displayField: "phone" })).toBe("设计组-主号");
  });

  it("备注为空（含纯空白）时回退昵称 → 邮箱 → uid → id → 未命名账号", () => {
    expect(displayName({ ...base, note: "   " })).toBe("林小凤");
    expect(displayName({ email: "a@b.com", uid: "u1", id: "i1" })).toBe("a@b.com");
    expect(displayName({ uid: "u1", id: "i1" })).toBe("u1");
    expect(displayName({ id: "i1" })).toBe("i1");
    expect(displayName({})).toBe("未命名账号");
    expect(displayName({ nickname: "", note: null, email: null, uid: null, id: null })).toBe(
      "未命名账号",
    );
  });

  it("清空备注后回退到昵称（卡片 / 下拉 / 积分页同名）", () => {
    expect(displayName({ ...base, note: "设计组-主号" })).toBe("设计组-主号");
    expect(displayName({ ...base, note: null })).toBe("林小凤");
  });

  it("accountIdentity 对邮箱本地段脱敏（只留首字符，至少 3 个星号）", () => {
    expect(accountIdentity({ email: "alice@example.com" })).toBe("a****@example.com");
    expect(accountIdentity({ email: "a@example.com" })).toBe("a***@example.com");
  });

  it("accountIdentity 对无域名邮箱原样返回", () => {
    expect(accountIdentity({ email: "not-an-email" })).toBe("not-an-email");
  });

  it("accountIdentity 无邮箱时回退 UID / ID", () => {
    expect(accountIdentity({ uid: "u1" })).toBe("UID · u1");
    expect(accountIdentity({ id: "i1" })).toBe("ID · i1");
  });
});
