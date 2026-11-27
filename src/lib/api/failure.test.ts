import { describe, expect, it } from "vitest";

import {
  ApiFailure,
  actionForKind,
  asError,
  asFailure,
  failureActionOf,
  failureDiagnostic,
  failureFromResponse,
  parseFailurePayload,
  structuredFailureOf,
} from "./index";

/**
 * 后端 `failure_response` 的真实形状（见 core 的 `failure.rs`）：
 * **只有四个字段**，动作是界面侧的映射，不在协议里。
 */
const GATEWAY_BODY = {
  code: 502,
  message: "服务端返回了网页而不是数据，可能是代理或网关拦截",
  failureKind: "gateway_html",
  httpStatus: 502,
};

describe("结构化失败通道", () => {
  it("asError() 对 ApiFailure 返回与改造前逐字一致的文案", () => {
    // 改造前：`throw new Error(data.message || data.error || 请求失败 (status))`。
    const failure = failureFromResponse(502, GATEWAY_BODY);
    expect(asError(failure)).toBe("服务端返回了网页而不是数据，可能是代理或网关拦截");
    // 老代码把抛出值当 Error 用（`instanceof Error`），必须依旧成立。
    expect(failure).toBeInstanceOf(Error);
    expect(failure.name).toBe("ApiFailure");
  });

  it("asError() 的既有怪癖原样保留（不得顺手修）", () => {
    // `JSON.stringify(e ?? "未知错误")` → null / undefined 得到**带引号**的字符串。
    expect(asError(null)).toBe('"未知错误"');
    expect(asError(undefined)).toBe('"未知错误"');
    expect(asError("缺少 accountId")).toBe("缺少 accountId");
    expect(asError(500)).toBe("500");
  });

  it("失败响应缺 message / error 时回落到状态码文案（与改造前同序）", () => {
    expect(asError(failureFromResponse(500, {}))).toBe("请求失败 (500)");
    expect(asError(failureFromResponse(400, { error: "缺少 accountId" }))).toBe("缺少 accountId");
    expect(asError(failureFromResponse(400, { message: "自定义", error: "忽略" }))).toBe("自定义");
  });

  it("httpCall 的非 2xx 分支保留四字段，动作由档位在前端映射", () => {
    const failure = failureFromResponse(502, GATEWAY_BODY);
    expect(failure.failureKind).toBe("gateway_html");
    expect(failure.httpStatus).toBe(502);
    expect(failure.code).toBe(502);
    expect(failure.action).toBe("check_proxy");
  });

  it("响应体没带 httpStatus 时用响应本身的状态码补齐", () => {
    const failure = failureFromResponse(401, { code: 401, failureKind: "login_expired" });
    expect(failure.httpStatus).toBe(401);
    expect(failure.action).toBe("relogin");
  });

  it("四档动作映射固定，upstream 没有动作（不留「点了没用」的按钮）", () => {
    expect(actionForKind("login_expired")).toBe("relogin");
    expect(actionForKind("gateway_html")).toBe("check_proxy");
    expect(actionForKind("malformed")).toBe("copy_diagnostic");
    expect(actionForKind("upstream")).toBeNull();
    expect(actionForKind(null)).toBeNull();

    const upstream = failureFromResponse(400, {
      code: 400,
      message: "账号不存在",
      failureKind: "upstream",
    });
    expect(upstream.action).toBeNull();
    expect(failureActionOf(upstream)).toBeNull();
  });

  it("没有档位就没有动作（老后端 / 非 HTTP 失败都不能凭空造按钮）", () => {
    const plain = failureFromResponse(400, { error: "缺少 accountId" });
    expect(plain.failureKind).toBeNull();
    // httpStatus / code 仍在（来自响应本身），但不足以推导动作。
    expect(plain.httpStatus).toBe(400);
    expect(plain.action).toBeNull();

    const oauthFailure = asFailure(new Error("登录超时，请重试"));
    expect(oauthFailure.failureKind).toBeNull();
    expect(oauthFailure.action).toBeNull();
  });

  it("未知档位一律忽略，不猜也不炸", () => {
    expect(parseFailurePayload({ failureKind: "some_future_kind" })).toBeNull();
    const failure = failureFromResponse(500, { failureKind: "some_future_kind" });
    expect(failure.failureKind).toBeNull();
    expect(failure.action).toBeNull();
  });

  it("Tauri 的 Err(String) 只有真的是结构化 JSON 才还原，普通字符串原样放行", () => {
    const structured = structuredFailureOf(JSON.stringify(GATEWAY_BODY));
    expect(structured).toBeInstanceOf(ApiFailure);
    expect(structured?.failureKind).toBe("gateway_html");
    expect(structured?.action).toBe("check_proxy");

    // 「原样放行」= call() 会把原始值继续抛给既有 catch 分支（零行为变化）。
    expect(structuredFailureOf("缺少 accountId")).toBeNull();
    expect(structuredFailureOf("请求失败 (500)")).toBeNull();
    expect(structuredFailureOf(new Error("渲染崩溃"))).toBeNull();
    expect(structuredFailureOf({ foo: "bar" })).toBeNull();
  });

  it("asFailure() 对任意抛出值都能给出文案，且与 asError() 口径一致", () => {
    for (const value of ["缺少 accountId", new Error("渲染崩溃"), { a: 1 }, null, 42]) {
      expect(asFailure(value).message).toBe(asError(value));
    }
    expect(asFailure(JSON.stringify(GATEWAY_BODY)).failureKind).toBe("gateway_html");
  });

  it("诊断摘要只含档位 / 状态码 / 文案，不含正文", () => {
    const text = failureDiagnostic(failureFromResponse(502, GATEWAY_BODY), {
      source: "credit-stats",
      host: "desktop",
    });
    expect(text).toContain("失败档位: gateway_html");
    expect(text).toContain("HTTP 状态: 502");
    expect(text).toContain("来源: credit-stats");
    expect(text).not.toContain("<html");
    expect(text).not.toContain("<!DOCTYPE");
  });
});
