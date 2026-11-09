// fe.portal.v1 page API (docs/spec/portal-bundle/ipc-v1.md §4); host-served to Admitted bundles only.
(() => {
  "use strict";
  const DOCUMENT_TOKEN = "__FE_DOCUMENT_TOKEN__";
  const API = "fe.portal.v1";
  const MAX_ENVELOPE_BYTES = 65536;
  const MESSAGES_PER_SECOND = 50;
  const MAX_PENDING = 64;
  const REPLY_TIMEOUT_MS = 10000;

  const webview = window.chrome && window.chrome.webview;
  if (!webview || window.fe) return;
  const post = webview.postMessage.bind(webview);
  const encoder = new TextEncoder();
  const pending = new Map();
  const deliveries = new Map();
  let nextId = 1;
  let tokens = MESSAGES_PER_SECOND;
  let refilledAt = performance.now();

  class FePortalError extends Error {
    constructor(code, message) {
      super(message);
      this.name = "FePortalError";
      this.code = code;
    }
  }

  const takeToken = () => {
    const now = performance.now();
    tokens = Math.min(MESSAGES_PER_SECOND, tokens + ((now - refilledAt) / 1000) * MESSAGES_PER_SECOND);
    refilledAt = now;
    if (tokens < 1) return false;
    tokens -= 1;
    return true;
  };

  // `v`, `n` and `id` lead every envelope, in that order: the host verifies them as a byte prefix.
  const request = (type, fields, onAccepted) =>
    new Promise((resolve, reject) => {
      if (pending.size >= MAX_PENDING || !takeToken()) {
        reject(new FePortalError("rate_limited", "Too many requests"));
        return;
      }
      const id = nextId++;
      const envelope = { v: 1, n: DOCUMENT_TOKEN, id, t: type };
      for (const [key, value] of Object.entries(fields)) envelope[key] = value;
      let text;
      try {
        text = JSON.stringify(envelope);
      } catch (error) {
        reject(new FePortalError("invalid", "Message is not serializable"));
        return;
      }
      if (encoder.encode(text).length > MAX_ENVELOPE_BYTES) {
        reject(new FePortalError("invalid", "Message is too large"));
        return;
      }
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new FePortalError("timeout", "No reply from the host"));
      }, REPLY_TIMEOUT_MS);
      pending.set(id, { resolve, reject, timer, onAccepted });
      post(text);
    });

  webview.addEventListener("message", (event) => {
    const message = event.data;
    if (!message || typeof message !== "object" || message.v !== 1 || message.n !== DOCUMENT_TOKEN) return;
    if (typeof message.re === "number") {
      const entry = pending.get(message.re);
      if (!entry) return;
      pending.delete(message.re);
      clearTimeout(entry.timer);
      if (message.ok) {
        if (entry.onAccepted) entry.onAccepted(message.data);
        entry.resolve(message.data);
      } else {
        const error = message.error || {};
        entry.reject(new FePortalError(String(error.code), String(error.message)));
      }
      return;
    }
    if (typeof message.sub === "number") {
      const onDelivery = deliveries.get(message.sub);
      if (!onDelivery) return;
      if (message.kind === "ended") deliveries.delete(message.sub);
      try {
        onDelivery({ kind: message.kind, seq: message.seq, data: message.data, dropped: message.dropped || 0 });
      } catch (error) {
        setTimeout(() => {
          throw error;
        });
      }
    }
  });

  const hello = () => request("hello", {});

  const subscribe = (stream, params, onDelivery) => {
    if (typeof onDelivery !== "function") {
      return Promise.reject(new FePortalError("invalid", "onDelivery must be a function"));
    }
    // Registered before the promise settles, so the first delivery is never missed.
    const accept = (data) => deliveries.set(data.sub, onDelivery);
    return request("subscribe", { stream: String(stream), params: params || {} }, accept).then((data) => ({
      id: data.sub,
      unsubscribe: () => {
        deliveries.delete(data.sub);
        return request("unsubscribe", { sub: data.sub }).then(() => undefined);
      },
    }));
  };

  const act = ({ op, node_id, key, value }) =>
    request("act", { op, node_id, key, value: value === undefined ? null : value });

  const invoke = ({ package: packageId, command, args }) =>
    request("invoke", { package: packageId, command, args: args === undefined ? {} : args }).then((data) => data.result);

  const emit = (stream, data) =>
    request("emit", { stream, data: data === undefined ? null : data }).then(() => undefined);

  const intent = (target) => {
    const names = Object.keys(target || {});
    if (names.length !== 1) return Promise.reject(new FePortalError("invalid", "Name exactly one intent"));
    return request("intent", { intent: names[0], node_id: target[names[0]] }).then(() => undefined);
  };

  const api = Object.freeze({ api: API, hello, subscribe, act, invoke, emit, intent, FePortalError });
  Object.defineProperty(window, "fe", { value: api, writable: false, enumerable: false, configurable: false });
})();
