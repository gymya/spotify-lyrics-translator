<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useApp } from './store'
import { usePlaybackControls } from './playbackControls'
import type { Settings } from './types'

const app = useApp()
const overlay = new URLSearchParams(location.search).has('overlay')
const draft = ref<Settings | null>(null)
const apiKey = ref('')
const azureRegionInput = ref('')
const hasKey = ref<boolean | null>(null)
const notice = ref('')
const busy = ref(false)
const choosingLyrics = ref(false)
const s = computed(() => app.snapshot)
const { position, playing, pending, draggedPosition, syncError, seek: seekTo, toggle } = usePlaybackControls(() => s.value?.playback, action)
watch(syncError, error => { if (error) notice.value = error })
const provider = computed(() => draft.value?.translationProvider ?? 'google')
const serviceName = computed(() => provider.value === 'azure' ? 'Microsoft Azure Translator' : 'Google Cloud Translation')
watch(provider, async () => {
  apiKey.value = ''; hasKey.value = null
  await refreshKeyStatus()
})
async function refreshKeyStatus() {
  if (overlay || !app.desktop) return
  const selected = provider.value
  try {
    const value = await invoke<boolean>('key_status', { provider: selected })
    if (selected === provider.value) hasKey.value = value
  } catch (error) { notice.value = String(error) }
}
const track = computed(() => s.value?.playback.track)
const previewStyle = computed(() => ({ '--subtitle-background': (draft.value?.subtitleBackground ?? s.value?.settings.subtitleBackground ?? true) ? 'rgb(0 0 0 / 70%)' : 'transparent', '--original-color': draft.value?.originalColor ?? s.value?.settings.originalColor, '--translated-color': draft.value?.translatedColor ?? s.value?.settings.translatedColor, opacity: draft.value?.opacity ?? s.value?.settings.opacity ?? 0.92, fontSize: `${draft.value?.fontSize ?? s.value?.settings.fontSize ?? 26}px` }))
const overlayStyle = computed(() => ({ '--subtitle-background': (s.value?.settings.subtitleBackground ?? true) ? 'rgb(0 0 0 / 70%)' : 'transparent', '--original-color': s.value?.settings.originalColor, '--translated-color': s.value?.settings.translatedColor, opacity: s.value?.settings.opacity ?? 0.92, fontSize: `${s.value?.settings.fontSize ?? 26}px` }))
function time(ms: number) { const seconds = Math.floor(ms / 1000); return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}` }
watch(() => s.value?.settings, value => {
  if (value && !draft.value) {
    draft.value = { ...value }
  }
})
watch(draft, (value, previous, onCleanup) => {
  if (!value || !previous || overlay) return
  const settings = { ...value }
  const timer = setTimeout(() => {
    invoke('save_settings', { settings }).catch(error => { notice.value = `無法儲存設定：${String(error)}` })
  }, 100)
  onCleanup(() => clearTimeout(timer))
}, { deep: true })
async function action(command: string, args?: Record<string, unknown>, success = '') {
  busy.value = true; notice.value = ''
  try { await invoke(command, args); notice.value = success; return true }
  catch (error) { notice.value = String(error); return false }
  finally { busy.value = false }
}
watch(() => track.value?.id, () => { choosingLyrics.value = false })
watch(() => s.value?.lyrics?.kind, kind => {
  if (kind === 'candidates') choosingLyrics.value = true
  else if (kind && kind !== 'error') choosingLyrics.value = false
})
async function toggleLyrics() {
  if (choosingLyrics.value) { choosingLyrics.value = false; return }
  choosingLyrics.value = true
  if (track.value && s.value?.lyrics?.kind !== 'candidates') {
    await action('search_lyrics', { trackId: track.value.id })
  }
}
async function saveKey(remove = false) {
  if (!remove && !apiKey.value.trim()) { notice.value = '請先輸入 API key'; return }
  const saved = await action('save_api_key', { key: remove ? '' : apiKey.value, provider: provider.value }, remove ? '已移除憑證；既有快取仍可使用' : '已安全儲存至 Keychain')
  if (saved) {
    apiKey.value = ''
    azureRegionInput.value = ''
  }
  await refreshKeyStatus()
}
async function seek(event: Event) {
  if (!track.value) return
  await seekTo(Number((event.target as HTMLInputElement).value))
}
async function drag(event: PointerEvent) {
  if (s.value?.editing && event.button === 0) {
    try { await invoke('drag_overlay') } catch { /* Drag can end before the native window receives it. */ }
  }
}
onMounted(() => {
  document.documentElement.classList.toggle('overlay-page', overlay)
  app.poll()
  void refreshKeyStatus()
})
onUnmounted(app.stop)
</script>

<template>
  <main v-if="overlay" class="overlay" :class="{ editing: s?.editing }" @pointerdown="drag">
    <div v-if="s?.editing" class="drag-label">拖曳調整位置</div>
    <div class="lyric-pair" :style="overlayStyle">
      <template v-if="s?.line && !app.error">
        <p class="original">{{ s.line.original }}</p>
        <p class="translated">{{ s.line.translated || (s.translating ? '正在翻譯歌詞…' : '') }}</p>
      </template>
      <p v-else class="status-text">{{ app.error || app.message }}</p>
    </div>
  </main>
  <main v-else class="dashboard">
    <header class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3"><div class="brand-mark">♫</div><div><p class="eyebrow">SPOTIFY LYRICS TRANSLATOR</p></div></div>
      <span class="badge"><i :class="{ active: playing }"></i>{{ playing ? '播放中' : track ? '已暫停' : '等待連線' }}</span>
    </header>

    <section class="now-playing card">
      <div class="flex items-start gap-4"><div class="album-mark" aria-hidden="true">♫</div><div class="min-w-0 flex-1"><p class="eyebrow">NOW PLAYING</p><h2 class="track-title">{{ track?.title || '準備好你的下一首歌' }}</h2><p class="muted">{{ track ? `${track.artist} · ${track.album}` : '開啟 Spotify 桌面版，讓翻譯跟著音樂走。' }}</p></div></div>
      <div class="flex items-center justify-center gap-3 mt-4" role="group" aria-label="Spotify 播放控制">
        <button class="secondary playback-button" title="上一首" aria-label="上一首" :disabled="busy || pending || !app.desktop || !track || s?.playback.status === 'unavailable'" @click="action('control_playback', { action: 'previous' })">
          <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M5 5h3v14H5zM19 5v14L9 12z" /></svg>
        </button>
        <button class="primary playback-button" :title="playing ? '暫停' : '播放'" :aria-label="playing ? '暫停' : '播放'" :disabled="busy || pending || !app.desktop || !s || s.playback.status === 'unavailable'" @click="toggle">
          <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path v-if="playing" d="M6 5h4v14H6zM14 5h4v14h-4z" /><path v-else d="M8 5v14l11-7z" /></svg>
        </button>
        <button class="secondary playback-button" title="下一首" aria-label="下一首" :disabled="busy || pending || !app.desktop || !track || s?.playback.status === 'unavailable'" @click="action('control_playback', { action: 'next' })">
          <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M16 5h3v14h-3zM5 5l10 7-10 7z" /></svg>
        </button>
      </div>
      <input class="progress" type="range" aria-label="播放進度" :aria-valuetext="time(position)" min="0" :max="track?.durationMs || 1" step="1000" :value="position" :disabled="busy || pending || !app.desktop || !track || s?.playback.status === 'unavailable'" @input="draggedPosition = Number(($event.target as HTMLInputElement).value)" @change="seek" @pointercancel="draggedPosition = null">
      <div class="flex justify-between time-label"><span>{{ time(position) }}</span><span>{{ time(track?.durationMs || 0) }}</span></div>
      <div class="preview lyric-pair" :style="previewStyle">
        <template v-if="s?.line"><p class="original">{{ s.line.original || '♪' }}</p><p class="translated">{{ s.line.translated || (s.translating ? '正在翻譯歌詞…' : '') }}</p></template>
        <p v-else class="status-text">{{ app.error || app.message }}</p>
      </div>
      <div class="flex items-center justify-between gap-3"><p class="small muted">{{ s?.translationMessage || (s?.translating ? '正在翻譯歌詞…' : `LRCLIB 歌詞 · ${serviceName} · 繁體中文`) }}</p><button :disabled="busy || !app.desktop" class="text-button shrink-0" @click="action('retry', undefined, '已安排重新載入')">重新載入 ↻</button></div>
      <div v-if="track" class="mt-4">
        <button class="text-button" :disabled="busy || s?.loading" @click="toggleLyrics">{{ choosingLyrics ? '關閉選擇其他版本' : '選擇其他歌詞版本' }}</button>
      </div>
      <div v-if="choosingLyrics && s?.lyrics?.kind === 'candidates'" class="candidate-list mt-4">
        <h2>選擇正確的歌詞版本</h2>
        <p class="small muted mt-2">以下版本與歌曲長度相差不超過 2 秒。請確認歌手與專輯；選擇後會記住對應，並使用已設定的翻譯服務。</p>
        <ul class="mt-3">
          <li v-for="candidate in s.lyrics.candidates" :key="candidate.id" class="candidate-row">
            <div class="min-w-0">
              <p class="candidate-title">{{ candidate.title }}</p>
              <p class="small muted">{{ candidate.artist }} · {{ candidate.album || '未提供專輯' }}</p>
              <p class="small muted">{{ time(candidate.durationMs) }} · {{ candidate.kind === 'synced' ? '同步歌詞' : candidate.kind === 'unsynced' ? '僅文字歌詞' : '純音樂' }}</p>
            </div>
            <button class="secondary shrink-0" :disabled="busy || !track" @click="action('select_lyrics', { trackId: track!.id, candidateId: candidate.id }, '已記住這首歌的歌詞版本')">使用此版本</button>
          </li>
        </ul>
      </div>
    </section>

    <div class="settings-grid">
      <section class="card">
        <div class="section-heading"><h2>浮動歌詞</h2><span class="eyebrow">OVERLAY</span></div>
        <template v-if="draft">
          <label class="toggle-row"><span>顯示浮動視窗</span><input v-model="draft.overlayEnabled" type="checkbox"></label>
          <label class="range-label" for="opacity">不透明度 <span>{{ Math.round(draft.opacity * 100) }}%</span></label>
          <input id="opacity" v-model.number="draft.opacity" type="range" min="0.1" max="1" step="0.01">
          <label class="range-label" for="font">字體大小 <span>{{ draft.fontSize }} px</span></label>
          <input id="font" v-model.number="draft.fontSize" type="range" min="14" max="48" step="1">
          <label class="toggle-row"><span>顯示半透明黑底</span><input v-model="draft.subtitleBackground" type="checkbox"></label>
          <label class="toggle-row"><span>原文字體顏色</span><input v-model="draft.originalColor" type="color"></label>
          <label class="toggle-row"><span>譯文字體顏色</span><input v-model="draft.translatedColor" type="color"></label>
          <label class="range-label" for="offset">時間偏移 <span>{{ draft.offsetMs > 0 ? '+' : '' }}{{ draft.offsetMs }} ms</span></label>
          <input id="offset" v-model.number="draft.offsetMs" type="range" min="-10000" max="10000" step="100">
          <p class="small muted">正值提早顯示，負值延後顯示。</p>
          <label class="toggle-row"><span>滑鼠穿透</span><input v-model="draft.clickThrough" type="checkbox"></label>
          <label class="toggle-row"><span>保持置頂</span><input v-model="draft.alwaysOnTop" type="checkbox"></label>
          <div class="flex gap-3 mt-3"><button :disabled="busy" class="secondary flex-1" @click="action('set_editing', { editing: !s?.editing })">{{ s?.editing ? '完成調整' : '調整位置' }}</button></div>
        </template>
        <p v-else class="muted">{{ app.message }}</p>
      </section>

      <section class="card flex flex-col">
        <div class="section-heading"><h2>翻譯服務</h2><span class="eyebrow">TRANSLATION</span></div>
        <template v-if="draft">
          <label class="input-label" for="translation-provider">翻譯服務</label>
          <select id="translation-provider" v-model="draft.translationProvider" class="key-input" :disabled="busy || !app.desktop">
            <option value="google">Google Cloud Translation Basic v2</option>
            <option value="azure">Microsoft Azure Translator（支援 F0）</option>
          </select>
          <template v-if="provider === 'azure'">
            <label class="input-label mt-3" for="azure-region">Azure Region</label>
            <input id="azure-region" v-model.trim="azureRegionInput" @change="draft.azureRegion = azureRegionInput" class="key-input" placeholder="例如 eastasia；Global 資源可留空" autocomplete="off" spellcheck="false" :disabled="busy || !app.desktop">
            <p class="small muted mt-2">填入 Azure「金鑰與端點」中的位置代碼；區域資源必填。</p>
          </template>
        </template>
        <label class="input-label" for="api-key">API key <span class="key-status">{{ hasKey === true ? '已儲存於 Keychain' : hasKey === false ? '尚未設定' : '' }}</span></label>
        <input id="api-key" v-model="apiKey" class="key-input" type="password" :placeholder="`貼上你的 ${provider === 'azure' ? 'Azure' : 'Google'} API key`" autocomplete="off" spellcheck="false" :disabled="!app.desktop">
        <p class="small muted mt-2">憑證只存於 macOS 鑰匙圈，不寫入設定檔。</p>
        <div class="flex gap-3 mt-4"><button class="primary flex-1" :disabled="busy || !app.desktop" @click="saveKey()">儲存憑證</button><button class="secondary" :disabled="busy || !hasKey" @click="saveKey(true)">移除</button></div>
        <div class="note mt-5"><strong>開始前</strong><p v-if="provider === 'google'">啟用 Google Cloud Translation API 與計費，再儲存 API key 即可開始翻譯。</p><p v-else>建立 Azure Translator F0 資源，填入 Key 與 Region 即可。</p></div>
        <div class="mt-auto pt-5"><button class="text-button" :disabled="!app.desktop || busy" @click="action('open_automation_settings')">開啟 macOS 自動化權限設定 ↗</button><p class="small muted mt-2">若讀不到歌曲，請允許此 App 控制 Spotify。</p></div>
      </section>
    </div>
    <p v-if="notice || app.error" class="notice" role="status">{{ notice || app.error }}</p>
    <footer>關閉此視窗後，App 仍在選單列執行。</footer>
  </main>
</template>
