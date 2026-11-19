// Update mirror for Provider Gateway for Claude (Cloudflare Worker).
//
// For users who reach Cloudflare but not GitHub (or reach it slowly):
//   GET /latest.json                 -> latest.json of the newest GitHub release, with the
//                                       download URL rewritten to this worker
//   GET /download/<tag>/<asset>      -> the release asset, streamed from GitHub and cached
//
// It changes no bytes of the installer: the app checks the minisign signature from
// latest.json against the public key built into it, so a mirror cannot slip in a file.

const REPO = "d-ridelman/provider-gateway-for-claude";
const GITHUB = `https://github.com/${REPO}/releases`;
const ASSET = /^\/download\/(v[0-9][\w.-]*)\/([\w.-]+)$/;

export default {
  async fetch(request, env, ctx) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed", { status: 405 });
    }
    const url = new URL(request.url);

    if (url.pathname === "/latest.json") {
      const upstream = await fetch(`${GITHUB}/latest/download/latest.json`, {
        cf: { cacheTtl: 300, cacheEverything: true },
      });
      if (!upstream.ok) return new Response("latest.json not found", { status: upstream.status });
      const latest = await upstream.json();
      for (const platform of Object.values(latest.platforms ?? {})) {
        const match = /\/releases\/download\/([^/]+)\/([^/]+)$/.exec(platform.url ?? "");
        if (match) platform.url = `${url.origin}/download/${match[1]}/${match[2]}`;
      }
      return Response.json(latest, { headers: { "cache-control": "public, max-age=300" } });
    }

    const asset = ASSET.exec(url.pathname);
    if (asset) {
      const [, tag, name] = asset;
      // Release assets are immutable per tag: cache them for a day at the edge.
      const upstream = await fetch(`${GITHUB}/download/${tag}/${name}`, {
        cf: { cacheTtl: 86400, cacheEverything: true },
      });
      if (!upstream.ok) return new Response("asset not found", { status: upstream.status });
      const headers = new Headers(upstream.headers);
      headers.set("cache-control", "public, max-age=86400, immutable");
      return new Response(upstream.body, { status: 200, headers });
    }

    return new Response("Provider Gateway for Claude update mirror", { status: 404 });
  },
};
