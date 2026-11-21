import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { QuotaKey, UiLanguage } from './api';

export interface QuotaMetric
{
  key: QuotaKey;
  label: string;
  current: string;
  resetAt: number | null;
}

export interface PopupSelection extends QuotaMetric
{
  id: number;
  revision: number | null;
  language: UiLanguage;
  fontScale: number;
}

interface PopupTarget { selection: PopupSelection; top: number; focus: boolean }
interface PopupInteraction { id: number; kind: 'enter' | 'leave' | 'close' }
type PopupReport = { kind: 'ready' } | { kind: 'size'; id: number; height: number } | PopupInteraction;

let generation = 0;
let selectionId = 0;

export const quotaPopup = {
  nextId: () => ++selectionId,
  set: (target: PopupTarget | null) => invoke<void>('set_quota_popup', { generation: ++generation, target }),
  report: (report: PopupReport) => invoke<void>('report_quota_popup', { report }),
  onSelection: (receive: (selection: PopupSelection | null) => void) =>
    listen<PopupSelection | null>('quota-popup-selection', event => receive(event.payload)),
  onInteraction: (receive: (interaction: PopupInteraction) => void) =>
    listen<PopupInteraction>('quota-popup-interaction', event => receive(event.payload))
};
