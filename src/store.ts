import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke, isTauri } from '@tauri-apps/api/core'
import type { Snapshot } from './types'

export const useApp = defineStore('app', () => {
  const snapshot = ref<Snapshot | null>(null)
  const error = ref('')
  const desktop = isTauri()
  let timer: ReturnType<typeof setTimeout> | undefined
  let stopped = false
  const message = computed(() => {
    const s = snapshot.value
    if (!s) return desktop ? '正在讀取播放資訊…' : '請以桌面 App 啟動，才能連接 Spotify'
    if (s.playback.message) return s.playback.message
    if (s.loading) return '正在載入歌詞…'
    switch (s.lyrics?.kind) {
      case 'candidates': return '找到可能的歌詞版本，請在設定頁選擇'
      case 'not_found': return '目前找不到這首歌曲的歌詞'
      case 'unsynced': return '這首歌曲目前沒有逐行同步歌詞'
      case 'instrumental': return '這是一首純音樂作品'
      case 'error': return `歌詞載入失敗：${s.lyrics.message}`
      case 'synced': return s.line ? '' : '等待歌詞開始…'
      default: return '等待 Spotify 播放音樂…'
    }
  })
  async function poll() {
    if (!desktop || stopped) return
    try { snapshot.value = await invoke<Snapshot>('get_snapshot'); error.value = '' }
    catch { error.value = '無法連接本機核心，請重新啟動 App'; snapshot.value = null }
    if (!stopped) timer = setTimeout(poll, location.search.includes('overlay') ? 150 : 500)
  }
  function stop() { stopped = true; clearTimeout(timer) }
  return { snapshot, error, message, desktop, poll, stop }
})
