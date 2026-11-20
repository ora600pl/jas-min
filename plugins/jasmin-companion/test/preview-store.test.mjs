import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { PreviewStore } from '../preview-store.mjs';

const version = 'a'.repeat(64);
const names = ['raport.html', 'wykresy/sql-1-1.html', 'wykresy/plotly.min.js'];
function asset(text) {
  const buffer = Buffer.from(text);
  return { buffer, sha256: createHash('sha256').update(buffer).digest('hex') };
}

test('capture publishes all verified assets together and coalesces repeated requests', async t => {
  const store = new PreviewStore();
  t.after(() => store.close());
  const started = Promise.withResolvers(), finish = Promise.withResolvers();
  let reads = 0;
  const upstream = async name => {
    reads++;
    if (name === 'wykresy/plotly.min.js') { started.resolve(); await finish.promise; }
    return asset(name);
  };
  const first = store.capture('client', version, names, upstream);
  await started.promise;
  assert.equal(store.has('client', version, 'raport.html'), false);
  assert.equal(await store.read('client', version, 'raport.html'), null);
  const repeat = store.capture('client', version, [...names].reverse(), upstream);
  finish.resolve();
  await Promise.all([first, repeat]);
  await store.capture('client', version, names, () => assert.fail('Unexpected upstream request'));
  assert.equal(reads, names.length);
  for (const name of names) {
    assert.equal(store.has('client', version, name), true);
    assert.deepEqual(await store.read('client', version, name), asset(name));
  }
  assert.equal(store.has('combined', version, names[0]), false);
  assert.equal(await store.read('cpu', version, names[0]), null);
  assert.equal(await store.read('client', version, 'report.zip'), null);
  await assert.rejects(store.capture('client', version, ['raport.html'], upstream), /asset list changed/);
});

test('a failed or corrupt edition leaves previously captured editions readable and can be retried', async t => {
  const store = new PreviewStore();
  t.after(() => store.close());
  const newer = 'b'.repeat(64);
  await store.capture('client', version, names, async name => asset('first ' + name));
  await assert.rejects(store.capture('client', newer, names, async name => ({
    ...asset(name), sha256: 'c'.repeat(64),
  })), /checksum/);
  assert.equal(await store.read('client', newer, names[0]), null);
  assert.deepEqual(await store.read('client', version, names[0]), asset('first ' + names[0]));
  await store.capture('client', newer, names, async name => asset('second ' + name));
  assert.deepEqual(await store.read('client', newer, names[2]), asset('second ' + names[2]));
  assert.deepEqual(await store.read('client', version, names[2]), asset('first ' + names[2]));
});

test('rejects unsafe manifests before transfer and waits for in-flight capture on close', async () => {
  const store = new PreviewStore();
  for (const name of ['report.zip', 'recipe.json', 'wykresy/../report.css', '/index.html', 'dir\\sql.html', 'A\nindex.html']) {
    await assert.rejects(store.capture('client', version, [name], () => assert.fail('unexpected transfer')), /Invalid/);
  }
  const started = Promise.withResolvers(), finish = Promise.withResolvers();
  const pending = store.capture('client', version, ['raport.html'], async () => {
    started.resolve(); await finish.promise; return asset('');
  });
  await started.promise;
  const closing = store.close();
  await assert.rejects(store.capture('client', version, ['raport.html'], () => assert.fail()), /closed/);
  finish.resolve();
  await Promise.all([pending, closing]);
  assert.equal(await store.read('client', version, 'raport.html'), null);
});
