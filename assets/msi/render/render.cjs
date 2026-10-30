/*
 * 把 assets/msi/render 下的两个页面渲染成 PNG。
 *
 * 前置：先起一个带调试端口的无头 Edge（sandbox 允许时）：
 *   msedge --headless --no-sandbox --disable-gpu --no-first-run \
 *          --force-device-scale-factor=1 --remote-debugging-port=9223 \
 *          --user-data-dir=.edge-tmp file:///<repo>/assets/msi/render/banner.html
 *
 * 用法：node assets/msi/render/render.cjs
 * 产物：assets/msi/render/banner.png、assets/msi/render/dialog.png
 * 之后用 System.Drawing 转 24bpp BMP 覆盖 assets/msi/banner.bmp / dialog.bmp。
 */
const fs = require("fs");
const path = require("path");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const here = __dirname;
const pages = [
  ["banner.html", 493, 58, "banner.png"],
  ["dialog.html", 493, 312, "dialog.png"],
];

(async () => {
  let target = null;
  for (let attempt = 0; attempt < 60 && !target; attempt += 1) {
    try {
      const list = await (await fetch("http://127.0.0.1:9223/json/list")).json();
      target = list.find((item) => item.type === "page" && item.url.startsWith("file://"));
    } catch (error) {
      await sleep(300);
    }
  }
  if (!target) throw new Error("找不到调试目标：确认 Edge 已带 --remote-debugging-port=9223 启动");

  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });

  let id = 1;
  const call = (method, params) =>
    new Promise((resolve, reject) => {
      const current = id++;
      const handler = (event) => {
        const message = JSON.parse(String(event.data));
        if (message.id !== current) return;
        ws.removeEventListener("message", handler);
        if (message.error) reject(new Error(JSON.stringify(message.error)));
        else resolve(message.result);
      };
      ws.addEventListener("message", handler);
      ws.send(JSON.stringify({ id: current, method, params }));
    });

  for (const [page, width, height, out] of pages) {
    await call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await call("Page.navigate", { url: `file://${path.join(here, page).replace(/\\/g, "/")}` });
    await sleep(1200);
    const shot = await call("Page.captureScreenshot", {
      format: "png",
      clip: { x: 0, y: 0, width, height, scale: 1 },
    });
    const file = path.join(here, out);
    fs.writeFileSync(file, Buffer.from(shot.data, "base64"));
    console.log("rendered", out, `${width}x${height}`);
  }

  await call("Emulation.clearDeviceMetricsOverride", {});
  ws.close();
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
