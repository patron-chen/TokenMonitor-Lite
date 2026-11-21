import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { mount } from 'svelte';
import App from './App.svelte';
import QuotaHistoryPopup from './QuotaHistoryPopup.svelte';
import './styles.css';

const component = isTauri() && getCurrentWindow().label === 'quota-history' ? QuotaHistoryPopup : App;
mount(component, { target: document.getElementById('app')! });
