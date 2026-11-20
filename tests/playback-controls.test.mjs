import { test } from 'node:test'
import assert from 'node:assert/strict'
import { effectScope, nextTick, ref } from 'vue'
import { usePlaybackControls } from '../src/playbackControls.ts'

function setup(t) {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const state = ref({ track: { id: 'song' }, positionMs: 10000, isPlaying: true, status: 'playing' })
  let finish
  const scope = effectScope()
  const controls = scope.run(() => usePlaybackControls(() => state.value, () => new Promise(resolve => { finish = resolve })))
  t.after(() => scope.stop())
  return { state, controls, finish: result => finish(result), poll: async patch => { state.value = { ...state.value, ...patch }; await nextTick() } }
}

test('dragging and seeking ignore stale polls until Spotify confirms', async t => {
  const { controls: c, poll, finish } = setup(t)
  c.draggedPosition.value = 60000
  await poll({ positionMs: 11000 })
  assert.equal(c.position.value, 60000)
  const request = c.seek(60000)
  await poll({ positionMs: 12000 })
  assert.equal(c.position.value, 60000)
  finish(true); await request
  await poll({ positionMs: 13000 })
  assert.equal(c.position.value, 60000)
  await poll({ positionMs: 60500 })
  assert.equal(c.pending.value, false)
  assert.equal(c.position.value, 60500)
})

test('pause and resume update immediately and resist stale polls', async t => {
  const { controls: c, poll, finish } = setup(t)
  let request = c.toggle()
  assert.equal(c.playing.value, false)
  await poll({ isPlaying: true, positionMs: 11000 })
  assert.equal(c.playing.value, false)
  assert.equal(c.position.value, 10000)
  finish(true); await request
  await poll({ isPlaying: false, status: 'paused' })
  assert.equal(c.pending.value, false)
  request = c.toggle()
  assert.equal(c.playing.value, true)
  finish(true); await request
  await poll({ isPlaying: false })
  assert.equal(c.playing.value, true)
  await poll({ isPlaying: true, status: 'playing' })
  assert.equal(c.pending.value, false)
})

test('failed and timed-out commands restore actual state', async t => {
  const { controls: c, finish } = setup(t)
  let request = c.seek(60000)
  finish(false); await request
  assert.equal(c.position.value, 10000)
  request = c.toggle()
  finish(true); await request
  t.mock.timers.tick(5000)
  assert.equal(c.playing.value, true)
  assert.equal(c.pending.value, false)
  assert.match(c.syncError.value, /Spotify/)
})

test('switching songs clears pending state and ignores late command completion', async t => {
  const { controls: c, finish, poll } = setup(t)
  const request = c.seek(60000)
  await poll({ track: { id: 'other' }, positionMs: 0 })
  finish(true); await request
  assert.equal(c.position.value, 0)
  assert.equal(c.pending.value, false)
  t.mock.timers.tick(5000)
  assert.equal(c.syncError.value, '')
})
