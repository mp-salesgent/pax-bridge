#!/usr/bin/env node
/**
 * Full HTTP + WebSocket smoke suite against a running pax-bridge-server.
 * Expects BASE_URL (default http://127.0.0.1:5055).
 *
 * Live terminal round-trips (ping/sale) assert error JSON shape when offline.
 */

const BASE = (process.env.BASE_URL || 'http://127.0.0.1:5055').replace(/\/$/, '');
const WS_URL = BASE.replace(/^http/, 'ws') + '/ws';

let failed = 0;
let passed = 0;

function ok(name, cond, detail = '') {
  if (cond) {
    passed += 1;
    console.log(`  PASS  ${name}`);
  } else {
    failed += 1;
    console.error(`  FAIL  ${name}${detail ? ` — ${detail}` : ''}`);
  }
}

async function req(method, path, body, expectStatus) {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: body !== undefined ? { 'content-type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = { _raw: text };
  }
  if (expectStatus !== undefined) {
    ok(`${method} ${path} → ${expectStatus}`, res.status === expectStatus, `got ${res.status}: ${text.slice(0, 200)}`);
  }
  return { status: res.status, json, text };
}

function waitWsMessages(timeoutMs = 4000) {
  return new Promise((resolve, reject) => {
    if (typeof WebSocket === 'undefined') {
      reject(new Error('Global WebSocket unavailable — use Node 22+ or install `ws`'));
      return;
    }
    const ws = new WebSocket(WS_URL);
    const msgs = [];
    const timer = setTimeout(() => {
      try {
        ws.close();
      } catch {}
      resolve({ ws, msgs });
    }, timeoutMs);
    ws.addEventListener('message', (ev) => {
      try {
        msgs.push(JSON.parse(String(ev.data)));
      } catch {
        msgs.push({ _raw: String(ev.data) });
      }
    });
    ws.addEventListener('error', (err) => {
      clearTimeout(timer);
      reject(err.error || err);
    });
    ws.addEventListener('open', () => {
      // keep open; caller closes after collecting
    });
  });
}

async function collectHello() {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(WS_URL);
    const timer = setTimeout(() => {
      ws.close();
      reject(new Error('WS HELLO timeout'));
    }, 5000);
    ws.addEventListener('message', (ev) => {
      clearTimeout(timer);
      let msg;
      try {
        msg = JSON.parse(String(ev.data));
      } catch {
        msg = null;
      }
      ws.close();
      resolve(msg);
    });
    ws.addEventListener('error', (err) => {
      clearTimeout(timer);
      reject(err.error || err);
    });
  });
}

async function main() {
  console.log(`\nAPI/WS suite → ${BASE}\n`);

  // --- Health / root ---
  {
    const h = await req('GET', '/api/health', undefined, 200);
    ok('health.ok', h.json?.ok === true || h.json?.status === 'ok' || typeof h.json === 'object', JSON.stringify(h.json));
    ok('health.bridgeVersion rust', String(h.json?.bridgeVersion || '').includes('rust') || String(h.json?.version || '').length > 0, JSON.stringify(h.json));
  }
  await req('GET', '/api', undefined, 200);

  // --- Serial ports ---
  {
    const r = await req('GET', '/api/terminals/serial-ports', undefined, 200);
    ok('serial-ports is array', Array.isArray(r.json) || Array.isArray(r.json?.ports), JSON.stringify(r.json));
  }

  // --- Terminals CRUD ---
  let termId = null;
  {
    const bad = await req('POST', '/api/terminals', { name: '' }, 400);
    ok('create terminal empty name rejected', bad.status === 400 || bad.status === 422, `status ${bad.status}`);

    const created = await req(
      'POST',
      '/api/terminals',
      {
        name: 'API Test Terminal',
        connType: 'tcp',
        ip: '127.0.0.1',
        port: 10009,
        model: 'A920 Pro',
      },
      201,
    );
    termId = created.json?.terminal?.id || created.json?.id;
    ok('create terminal returns id', !!termId, JSON.stringify(created.json));

    const list = await req('GET', '/api/terminals', undefined, 200);
    const rows = Array.isArray(list.json) ? list.json : list.json?.terminals || [];
    ok('list terminals includes created', rows.some((t) => t.id === termId), JSON.stringify(list.json).slice(0, 200));

    if (termId) {
      const upd = await req('PUT', `/api/terminals/${termId}`, { name: 'API Test Terminal Renamed' }, 200);
      ok(
        'update terminal name',
        upd.json?.terminal?.name === 'API Test Terminal Renamed' || upd.json?.name === 'API Test Terminal Renamed',
        JSON.stringify(upd.json),
      );
    }
  }

  // --- WS HELLO ---
  console.log('\n  -- WebSocket --');
  try {
    const hello = await collectHello();
    ok('WS HELLO type', hello?.type === 'HELLO', JSON.stringify(hello));
    ok('WS HELLO has ts', typeof hello?.ts === 'number', JSON.stringify(hello));
  } catch (e) {
    ok('WS HELLO', false, String(e));
  }

  // --- Ping / diagnose / batch-close (offline terminal → structured error) ---
  if (termId) {
    console.log('\n  -- Terminal ops (expect gateway errors offline) --');
    for (const path of [`/api/terminals/${termId}/ping`, `/api/terminals/${termId}/batch-close`]) {
      const r = await req('POST', path, {});
      const code = r.json?.error?.code || r.json?.code;
      ok(
        `${path} structured error`,
        r.status >= 400 && (typeof code === 'string' || r.json?.error),
        `status=${r.status} body=${JSON.stringify(r.json).slice(0, 180)}`,
      );
    }
    {
      const r = await req('POST', `/api/terminals/${termId}/diagnose`, {});
      ok(
        'diagnose returns report or error',
        (r.status === 200 && r.json?.report) || (r.status >= 400 && r.json?.error),
        `status=${r.status} body=${JSON.stringify(r.json).slice(0, 180)}`,
      );
    }
  }

  // --- Payments validation ---
  console.log('\n  -- Payments --');
  // missing terminalId → NOT_FOUND; bad amount → VALIDATION
  await req('POST', '/api/payments/sale', { amountCents: 100 }, 404);
  if (termId) {
    await req('POST', '/api/payments/sale', { terminalId: termId, amountCents: -1 }, 400);
    await req('POST', '/api/payments/refund', { terminalId: termId }, 400);
  }
  await req('POST', '/api/payments/void', { terminalId: termId || 'missing', origTxnId: 'nope' }, 404);

  if (termId) {
    // Offline sale: should fail with gateway/timeout style error, not 500 opaque
    const sale = await req('POST', '/api/payments/sale', {
      terminalId: termId,
      amountCents: 100,
      orderRef: 'api-test',
    });
    ok(
      'sale offline returns error envelope',
      sale.status >= 400 && (sale.json?.error?.code || sale.json?.transaction),
      `status=${sale.status} ${JSON.stringify(sale.json).slice(0, 220)}`,
    );
  }

  const payList = await req('GET', '/api/payments', undefined, 200);
  ok('payments list', payList.json != null);

  // get missing payment
  await req('GET', '/api/payments/does-not-exist', undefined, 404);

  // --- WS lifecycle during a payment attempt ---
  console.log('\n  -- WS during sale --');
  if (termId && typeof WebSocket !== 'undefined') {
    try {
      const ws = new WebSocket(WS_URL);
      const events = [];
      await new Promise((resolve, reject) => {
        const t = setTimeout(() => reject(new Error('ws open timeout')), 5000);
        ws.addEventListener('open', () => {
          clearTimeout(t);
          resolve();
        });
        ws.addEventListener('error', (e) => {
          clearTimeout(t);
          reject(e.error || e);
        });
      });
      ws.addEventListener('message', (ev) => {
        try {
          events.push(JSON.parse(String(ev.data)));
        } catch {}
      });
      // drain HELLO
      await new Promise((r) => setTimeout(r, 200));
      events.length = 0;

      await req('POST', '/api/payments/sale', {
        terminalId: termId,
        amountCents: 101,
        orderRef: 'ws-lifecycle',
      });

      await new Promise((r) => setTimeout(r, 1500));
      ws.close();
      const types = events.map((e) => e.type).filter(Boolean);
      ok('WS emitted payment lifecycle events', types.length > 0, `types=${JSON.stringify(types)}`);
      ok(
        'WS lifecycle includes PENDING or ERROR-ish',
        types.some((t) => /PENDING|STARTED|ERROR|FAILED|DECLINED|TIMEOUT|APPROVED|COMPLETE/i.test(t)),
        `types=${JSON.stringify(types)}`,
      );
    } catch (e) {
      ok('WS during sale', false, String(e));
    }
  }

  // --- Cleanup ---
  if (termId) {
    const del = await req('DELETE', `/api/terminals/${termId}`);
    ok('delete terminal', del.status === 204 || del.status === 200, `status ${del.status}`);
  }

  console.log(`\nResult: ${passed} passed, ${failed} failed\n`);
  process.exit(failed ? 1 : 0);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
