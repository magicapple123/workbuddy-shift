import { describe, expect, it } from "vitest";

import { accountIdentity, displayName } from "./account-display";

describe("account-display", () => {
  it("displayName 按 displayField 取备注 / 手机号 / 账号名", () => {
    const base = { nickname: "昵称", uid: "u1", id: "i1", email: "a@b.com" };
    expect(displayName({ ...base, note: "主力号", displayField: "note" })).toBe("主力号");
    expect(displayName({ ...base, phoneNumber: "138 0000 0000", displayField: "phone" })).toBe(
      "138 0000 0000",
    );
    expect(displayName({ ...base, displayField: "nickname" })).toBe("昵称");
  });

  it("displayName 在偏好字段为空时回退 nickname → uid → id → 未命名账号", () => {
    expect(displayName({ nickname: "昵称", uid: "u1", displayField: "note" })).toBe("昵称");
    expect(displayName({ uid: "u1", id: "i1" })).toBe("u1");
    expect(displayName({ id: "i1" })).toBe("i1");
    expect(displayName({})).toBe("未命名账号");
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
