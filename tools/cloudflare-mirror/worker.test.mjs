// node --test tools/cloudflare-mirror/worker.test.mjs
import assert from "node:assert/strict";
import test from "node:test";
import worker from "./worker.js";

const latest = {
  version: "0.2.0",
  platforms: {
    "windows-x86_64": {
      signature: "sig",
      url: "https://github.com/d-ridelman/provider-gateway-for-claude/releases/download/v0.2.0/App_0.2.0_x64-setup.exe",
    },
  },
};

function withFetch(handler, body) {
  const original = globalThis.fetch;
  globalThis.fetch = handler;
  return body().finally(() => {
    globalThis.fetch = original;
  });
}

test("latest.json points downloads at the mirror, signature untouched", () =>
  withFetch(async (url) => {
    assert.match(String(url), /releases\/latest\/download\/latest\.json$/);
    return Response.json(latest);
  }, async () => {
    const res = await worker.fetch(new Request("https://mirror.example/latest.json"));
    const body = await res.json();
    const win = body.platforms["windows-x86_64"];
    assert.equal(win.url, "https://mirror.example/download/v0.2.0/App_0.2.0_x64-setup.exe");
    assert.equal(win.signature, "sig");
  }));

test("assets are proxied from the GitHub release of the same tag", () =>
  withFetch(async (url) => {
    assert.equal(String(url), "https://github.com/d-ridelman/provider-gateway-for-claude/releases/download/v0.2.0/App_0.2.0_x64-setup.exe");
    return new Response("installer-bytes");
  }, async () => {
    const res = await worker.fetch(new Request("https://mirror.example/download/v0.2.0/App_0.2.0_x64-setup.exe"));
    assert.equal(res.status, 200);
    assert.equal(await res.text(), "installer-bytes");
  }));

test("anything else is refused, no open proxy", async () => {
  for (const path of ["/download/../../x", "/download/v1/a/b", "/https://evil.example", "/"]) {
    const res = await worker.fetch(new Request(`https://mirror.example${path}`));
    assert.equal(res.status, 404, path);
  }
  const post = await worker.fetch(new Request("https://mirror.example/latest.json", { method: "POST" }));
  assert.equal(post.status, 405);
});
