import { computed, onScopeDispose, ref, watch } from 'vue'
import type { Playback } from './types'

type Target = { kind: 'seek' | 'toggle'; trackId: string | undefined; positionMs: number; isPlaying: boolean; ready: boolean }

export function usePlaybackControls(
  playback: () => Playback | undefined,
  send: (command: string, args: Record<string, unknown>) => Promise<boolean>,
) {
  const target = ref<Target | null>(null)
  const draggedPosition = ref<number | null>(null)
  const syncError = ref('')
  let timer: ReturnType<typeof setTimeout> | undefined
  const position = computed(() => draggedPosition.value ?? target.value?.positionMs ?? playback()?.positionMs ?? 0)
  const playing = computed(() => target.value?.isPlaying ?? playback()?.isPlaying ?? false)
  const pending = computed(() => target.value !== null)
  function clear() {
    clearTimeout(timer)
    target.value = null
  }
  let playbackTrackId = playback()?.track?.id
  watch(playback, current => {
    const requested = target.value
    if (!current || current.status === 'unavailable' || current.track?.id !== playbackTrackId) {
      draggedPosition.value = null
      clear()
    } else if (requested?.ready && (requested.kind === 'seek'
      ? Math.abs(current.positionMs - requested.positionMs) <= 1500
      : current.isPlaying === requested.isPlaying)) {
      clear()
    }
    playbackTrackId = current?.track?.id
  })
  async function request(next: Target, command: string, args: Record<string, unknown>) {
    if (pending.value) return
    syncError.value = ''
    target.value = next
    const requested = target.value
    const success = await send(command, args)
    if (target.value !== requested) return
    if (!success) { clear(); return }
    requested.ready = true
    // Keep the requested UI state until a subsequent Spotify poll confirms it.
    timer = setTimeout(() => {
      clear()
      syncError.value = 'Spotify 尚未確認操作，已恢復實際播放狀態，請重試。'
    }, 5000)
  }
  async function seek(positionMs: number) {
    const current = playback()
    if (!current?.track) { draggedPosition.value = null; return }
    const result = request({ kind: 'seek', trackId: current.track.id, positionMs, isPlaying: playing.value, ready: false },
      'seek_playback', { trackId: current.track.id, positionMs })
    draggedPosition.value = null
    await result
  }
  async function toggle() {
    const isPlaying = !playing.value
    await request({ kind: 'toggle', trackId: playback()?.track?.id, positionMs: position.value, isPlaying, ready: false },
      'control_playback', { action: isPlaying ? 'play' : 'pause' })
  }
  onScopeDispose(() => clearTimeout(timer))
  return { position, playing, pending, draggedPosition, syncError, seek, toggle }
}
