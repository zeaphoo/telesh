/**
 * 领域状态 -> 视觉语义。两个 app 各存一份,语义一致。
 *
 * tone 对应 CSS 扩展色(success / warning / info / danger / muted),
 * i18nKey 是 `common` 命名空间下的文案键(见 locales/<lang>/common.json)。
 */
const STATUS_MAP = {
  live: { tone: 'success', pulse: true },
  online: { tone: 'success', pulse: true },
  pending: { tone: 'warning', pulse: true },
  starting: { tone: 'warning', pulse: true },
  error: { tone: 'danger', pulse: false },
  target_gone: { tone: 'muted', pulse: false },
  offline: { tone: 'muted', pulse: false },
}

/** 未知状态回退成中性灰,不抛错(接口可能先于 UI 增加新状态)。 */
export function statusMeta(state) {
  const meta = STATUS_MAP[state] || { tone: 'muted', pulse: false }
  return { ...meta, key: `status.${state}` }
}
