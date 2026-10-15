/** This internal build has no verified update feed or signing key. */
export const INTERNAL_UPDATE_POLICY = {
  status: 'manual-install-required',
  title: '内部增强版更新',
  message: '当前为内部增强版，未检查最新内部版本。请向内部发布负责人获取经过确认的内部安装包，按内部发布说明手动安装。',
} as const;
