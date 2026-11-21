// 서버의 답(`page`와 `extra.view`)에서 그릴 화면을 고른다.
import type { Component } from 'svelte';
import type { PageData } from '../lib/types';
import Activity from './Activity.svelte';
import Automation from './Automation.svelte';
import Browser from './Browser.svelte';
import Character from './Character.svelte';
import CharacterRevision from './CharacterRevision.svelte';
import Connections from './Connections.svelte';
import Expressions from './Expressions.svelte';
import Handover from './Handover.svelte';
import Home from './Home.svelte';
import Memory from './Memory.svelte';
import MemoryEntry from './MemoryEntry.svelte';
import SettingsNotify from './SettingsNotify.svelte';
import SettingsSafety from './SettingsSafety.svelte';
import SettingsWork from './SettingsWork.svelte';
import SkillDetail from './SkillDetail.svelte';
import Skills from './Skills.svelte';
import Start from './Start.svelte';
import System from './System.svelte';
import Work from './Work.svelte';

type PageComponent = Component<{ data: PageData<any> }>;

export function pageFor(data: PageData): PageComponent {
  const view = (data.extra as { view?: string } | null)?.view;
  switch (data.page) {
    case 'overview':
      return Home;
    case 'work':
      return Work;
    case 'automation':
      return Automation;
    case 'activity':
      return Activity;
    case 'memory':
      return view === 'entry' || view === 'missing' ? MemoryEntry : Memory;
    case 'skills':
      return view === 'detail' ? SkillDetail : Skills;
    case 'character':
      return view === 'revision' || view === 'revision_missing' ? CharacterRevision : Character;
    case 'expressions':
      return Expressions;
    case 'connections':
      return Connections;
    case 'browser':
      return view === 'handover' ? Handover : Browser;
    case 'safety':
      return SettingsSafety;
    case 'notify':
      return SettingsNotify;
    case 'tasks':
      return SettingsWork;
    case 'system':
      return System;
    case 'start':
      return Start;
  }
}

/** 본문을 가득 쓰는 화면(원격 화면이 넓다). */
export function isWide(data: PageData): boolean {
  return data.page === 'browser' && (data.extra as { view?: string } | null)?.view === 'handover';
}
