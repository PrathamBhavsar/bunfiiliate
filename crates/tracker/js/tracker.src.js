(function (w, d) {
  var s = d.currentScript;
  var days = +(s && s.getAttribute("data-days")) || 30;
  var endpoint = s && s.src ? new URL(s.src).origin + "/c" : "";
  var q = new URLSearchParams(w.location.search);

  function get(name) {
    var m = ("; " + d.cookie).split("; " + name + "=");
    return m.length > 1 ? decodeURIComponent(m.pop().split(";")[0]) : "";
  }

  function set(name, value, ttlDays) {
    d.cookie =
      name + "=" + encodeURIComponent(value) +
      ";max-age=" + Math.round(ttlDays * 86400) +
      ";path=/;SameSite=Lax" +
      (w.location.protocol === "https:" ? ";Secure" : "");
  }

  function uid() {
    var c = w.crypto;
    if (c && c.randomUUID) return c.randomUUID();
    var b = c.getRandomValues(new Uint8Array(16));
    return Array.prototype.map
      .call(b, function (x) { return (x + 256).toString(16).slice(1); })
      .join("");
  }

  // `fresh`: the page was just entered through a link, so only what that URL carries is sent.
  // A stored bf_ref from an older visit must not ride along with a new bf_click: /r already
  // recorded that click, and a code beside it would be counted a second time.
  function track(code, click, fresh) {
    var vid = get("bf_vid");
    if (!vid) set("bf_vid", (vid = uid()), 365);
    if (!fresh) {
      code = code || get("bf_ref");
      click = click || get("bf_click");
    }
    if (!(code || click) || !endpoint) return;
    if (code) set("bf_ref", code, days);
    if (click) set("bf_click", click, days);
    var body = JSON.stringify({
      code: code || null,
      click: click || null,
      vid: vid,
      url: w.location.origin + w.location.pathname,
      ref: d.referrer && new URL(d.referrer).origin
    });
    // text/plain keeps this a "simple" cross-origin request: no preflight, so the beacon is
    // not dropped when the page and the tracker are on different origins. The server parses
    // the body whatever its Content-Type.
    var n = w.navigator;
    if (n.sendBeacon && n.sendBeacon(endpoint, new Blob([body], { type: "text/plain" }))) return;
    w.fetch(endpoint, {
      method: "POST",
      headers: { "content-type": "text/plain" },
      body: body,
      keepalive: true,
      credentials: "omit"
    }).catch(function () {});
  }

  w.bunfiiliate = { track: track };
  var ref = q.get("ref"), click = q.get("bf_click");
  if (ref || click) track(ref, click, true);
  else if (!get("bf_vid")) set("bf_vid", uid(), 365);
})(window, document);
