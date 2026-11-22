// 만든 파일이다(tools/gen-types.mjs). 손으로 고치지 않는다: 시험 데이터를 다시 만들고 `npm run types`.
// 열거형 값은 string으로만 적힌다. 값의 뜻과 가능한 값은 docs/derived.md와 데몬의 타입을 본다.

import type { Tone } from './types';

/** 화면이 보는 데이터 전체(데몬의 `admin::snapshot::Snapshot`). */
export interface Snapshot {
  access: {
    admin_url: string | null;
    external_blocked: boolean;
    gateway_url: string | null;
    session_external: boolean;
    totp_enabled: boolean;
    totp_pending: Array<string> | null;
  };
  activity: Array<{
    at: string;
    id: number;
    kind: string;
    run_id: unknown;
    summary: string;
    ui: {
      detail: string;
      groups: Array<string>;
      icon: string;
      label: string;
      tone: Tone;
    };
  }>;
  alerts: Array<{
    at: string;
    delivered: boolean;
    id: number;
    text: string;
  }>;
  app: {
    fcm_project: string | null;
    fcm_ready: boolean;
    notify_client: string | null;
    push_preview: boolean;
  };
  approval_ttl_hours: number;
  approvals: Array<{
    auto: boolean;
    card_ref: string;
    conversation: string;
    decided_at: unknown;
    expires_at: string;
    id: string;
    kind: string;
    payload: Record<string, unknown>;
    requested_at: string;
    result: string | null;
    run_id: string;
    status: string;
    summary: string;
    ui: {
      pending: boolean;
      status: string;
      tone: Tone;
    };
  }>;
  backup: {
    cron: string;
    enabled: boolean;
    keep_days: number;
    last: string;
    ui: {
      cron_text: string;
    };
  };
  browser: {
    blocked: Array<string>;
    idle_minutes: number;
    recent_blocks: Array<{
      at: string;
      blocked: string;
      host: string;
      port: number;
    }>;
    running: boolean;
    sandbox: string;
    sites: Array<{
      id: string;
      name: string;
      origin: string;
      saved: boolean;
      username: string;
    }>;
  } | null;
  checks: Array<{
    detail: string;
    name: string;
    ok: boolean;
  }>;
  checks_at: string | null;
  clients: Array<[{
    acked_seq: number;
    created_at: string;
    id: string;
    last_seen_at: string | null;
    name: string;
    push_at: string | null;
    push_platform: string | null;
    revoked_at: string | null;
    role: string;
    scopes: Array<string>;
    ui: {
      has_app_tools: boolean;
      has_browser_handover: boolean;
      has_owner_files: boolean;
      has_push_token: boolean;
      has_security_mode: boolean;
      is_app: boolean;
      is_notify_target: boolean;
      role_label: string;
      scope_labels: Array<string>;
    };
  }, boolean]>;
  compact_at_tokens: number | null;
  daily_cap: number | null;
  dev_allow_hosts: string;
  dev_cost_cap: number | null;
  dev_local_roots: string;
  discord: {
    content_refused: boolean;
    guild_calls: boolean;
    notify: string | null;
    owner: string | null;
    prefix: string | null;
    servers: {
      Err?: string;
      Ok?: Array<{
        admin: boolean;
        id: string;
        name: string;
        owner: boolean;
      }>;
    } | null;
    state: ({
      Connected?: {
        bot: string;
      };
      Stopped?: {
        reason: string;
      };
    } | string);
    token_set: boolean;
    ui: {
      bot: string | null;
      state: string;
      status: string;
      stopped_reason: string | null;
      tone: Tone;
    };
  };
  egress_block: boolean;
  expressions: {
    categories: Array<{
      icon: string;
      id: string;
      label: string;
      sort: number;
    }>;
    emoji: Array<{
      category: string;
      created_at: string;
      description: string;
      file: {
        animated: boolean;
        created_at: string;
        height: number;
        id: string;
        mime: string;
        size: number;
        unlisted_at: unknown;
        width: number;
      };
      id: string;
      kind: string;
      label: string;
      name: string;
      origin: string;
      tags: Array<string>;
      ui: {
        origin_label: string;
        size_text: string;
      };
      updated_at: string;
    }>;
    stickers: Array<{
      category: string;
      created_at: string;
      description: string;
      file: {
        animated: boolean;
        created_at: string;
        height: number;
        id: string;
        mime: string;
        size: number;
        unlisted_at: unknown;
        width: number;
      };
      id: string;
      kind: string;
      label: string;
      name: string;
      origin: string;
      tags: Array<string>;
      ui: {
        origin_label: string;
        size_text: string;
      };
      updated_at: string;
    }>;
    version: string;
  } | null;
  followups_closed: Array<{
    closed_at: string;
    conversation: unknown;
    created_at: string;
    detail: string;
    due_at: string;
    id: string;
    origin: string;
    result: string | null;
    run_id: unknown;
    status: string;
    task: string;
    ui: {
      origin_label: string;
      status_label: string;
      tone: Tone;
      uses_browser: boolean;
    };
  }>;
  followups_open: Array<{
    closed_at: unknown;
    conversation: unknown;
    created_at: string;
    detail: string;
    due_at: string;
    id: string;
    origin: string;
    result: unknown;
    run_id: unknown;
    status: string;
    task: string;
    ui: {
      origin_label: string;
      status_label: string;
      tone: Tone;
      uses_browser: boolean;
    };
  }>;
  gateway_binds: Array<string>;
  goals: Array<{
    children: Array<{
      by_review: boolean;
      child: string;
      goal: string;
      kind: string;
    }>;
    goal: {
      closed_at: string | null;
      conversation: unknown;
      created_at: string;
      deadline: string | null;
      id: string;
      interests: string;
      next_review_at: string | null;
      purpose: string;
      review_every_hours: number;
      status: string;
      success: string;
      title: string;
      updated_at: string;
    };
    log: Array<{
      at: string;
      kind: string;
      text: string;
    }>;
    ui: {
      open: boolean;
      status_label: string;
    };
  }>;
  google: {
    account: string | null;
    connected: boolean;
    consent_minutes_left: number | null;
    has_client: boolean;
    scopes: string | null;
    sources: Array<{
      label: string;
      state: {
        baseline_at: string;
        checked_at: string;
        cursor: string;
        last_error: string | null;
        source: string;
      } | null;
    }>;
    ui: {
      has_calendar_write: boolean;
      has_gmail_send: boolean;
      scopes: Array<{
        granted: boolean;
        label: string;
      }>;
      status: string;
      tone: Tone;
    };
  };
  images: {
    daily_cap: number;
    key_set: boolean;
    recent: Array<{
      background: string;
      cost_usd: number;
      created_at: string;
      error: string | null;
      file_id: unknown;
      finished_at: string;
      id: string;
      input_tokens: number;
      inputs: number;
      model: string;
      output_tokens: number;
      prompt: string;
      quality: string;
      run_id: unknown;
      size: string;
      status: string;
      ui: {
        label: string;
        tone: Tone;
      };
    }>;
    today_usd: number;
  };
  keep_days: number | null;
  kept_files: Array<{
    file: {
      conversation: unknown;
      created_at: string;
      direction: string;
      id: string;
      kind: string;
      mime: string;
      name: string;
      rel_path: string;
      run_id: unknown;
      sha256: string;
      size: number;
    };
    kept_at: string;
    note: string;
    title: string;
    ui: {
      direction_label: string;
      kind_label: string;
      size_text: string;
    };
  }>;
  last_backup: {
    at: string;
    ok: boolean;
    text: string;
  } | null;
  last_briefing: {
    at: string;
    ok: boolean;
    text: string;
  } | null;
  last_compact: {
    at: string;
    ok: boolean;
    text: string;
  } | null;
  last_precheck: {
    at: string;
    ok: boolean;
    text: string;
  } | null;
  last_remember: {
    at: string;
    ok: boolean;
    text: string;
  } | null;
  memories: Array<{
    body: string;
    created_at: string;
    revision: number;
    section: string;
    slug: string;
    summary: string;
    title: string;
    ui: {
      section_key: string;
      section_label: string;
    };
    updated_at: string;
  }>;
  notice_rules: Array<{
    created_at: string;
    key: string;
    label: string;
    rule: string;
    ui: {
      rule_label: string;
    };
  }>;
  now: string;
  nudges: Array<{
    created_at: string;
    due_at: string;
    goal: unknown;
    id: string;
    note: string;
    stage: number;
    status: string;
    title: string;
    ui: {
      stage_label: string;
    };
  }>;
  profile: {
    avatar: {
      by: string;
      height: number;
      id: string;
      mime: string;
      set_at: string;
      size: number;
      ui: {
        by_label: string;
        facts: string;
      };
      width: number;
    } | null;
    name: string;
    name_set: boolean;
  };
  quiet_busy: number;
  quiet_calendar: boolean;
  quiet_except: string;
  quiet_hours: string | null;
  rotate_tokens: number;
  routine_wakes: string | null;
  routines: Array<{
    next: string;
    routine: {
      conversation: unknown;
      created_at: string;
      failures: number;
      id: string;
      input: Record<string, unknown>;
      last_ok: boolean;
      last_run_at: string;
      last_text: string;
      name: string;
      next_at: unknown;
      runs: number;
      script: string | null;
      status: string;
      task: string;
      tools: Array<string>;
      trigger: {
        Change?: string;
        Cron?: string;
        Every?: {
          minutes: number;
          window: string;
        };
      };
      until: unknown;
      updated_at: string;
      wakes: number;
    };
    runs: Array<{
      at: string;
      ok: boolean;
      text: string;
      woke: boolean;
    }>;
    ui: {
      can_pause: boolean;
      can_run: boolean;
      resume_label: string | null;
      status_label: string;
      tone: Tone;
      trigger_label: string;
    };
  }>;
  schedules: Array<{
    action: string;
    enabled: boolean;
    id: string;
    kind: string;
    last_fired_at: string | null;
    max_delay_secs: number;
    next_due_at: string;
    payload: Record<string, unknown>;
    spec: string;
    timezone: string;
    ui: {
      cron_text: string;
      label: string;
    };
  }>;
  scripts_sandbox: string | null;
  security_mode: string;
  session: {
    cost_total_usd: number;
    created_at: string;
    id: string;
    last_compacted_at: unknown;
    last_context_tokens: number;
    last_used_at: string;
    role: string;
    runs_since_compact: number;
    sandbox: string;
    started: boolean;
    status: string;
    ui: {
      short_id: string;
    };
  } | null;
  setup: {
    complete: boolean;
    config_path: string | null;
    dismissed: boolean;
    env: Array<{
      command: string | null;
      detail: string;
      key: string;
      label: string;
      tone: Tone;
    }>;
    steps: Array<{
      icon: string;
      key: string;
      state: string;
      state_label: string;
      summary: string;
      title: string;
      tone: Tone;
    }>;
  };
  signals: {
    classify_daily: number;
    classify_used: number;
    consistency: boolean;
    enabled: boolean;
    evening: boolean;
    interests: string;
    poll_minutes: number;
    recent: Array<{
      decided_at: unknown;
      id: number;
      interest: string | null;
      key: string;
      payload: Record<string, unknown>;
      reason: string | null;
      received_at: string;
      source: string;
      summary: string;
      ui: {
        verdict_label: string;
      };
      verdict: string | null;
      woke_at: unknown;
    }>;
    wake_daily: number;
    wake_used: number;
    weekly: boolean;
  };
  site_grants: Array<{
    actions: boolean;
    created_at: string;
    expires_at: string | null;
    host: string;
    id: string;
    last_used_at: string;
    login: boolean;
    origin: string;
    purpose: string;
    revoked_at: string | null;
    ui: {
      active: boolean;
      origin_label: string;
      scope_text: string;
    };
    uses: number;
  }>;
  task_channel: string | null;
  tasks_closed: Array<{
    base: unknown;
    brief: string;
    closed_at: string;
    conversation: unknown;
    created_at: string;
    id: string;
    initiator: string;
    kind: string;
    repo: unknown;
    result: string;
    run_id: unknown;
    sandbox: unknown;
    session_id: unknown;
    source: string;
    status: string;
    thread: boolean;
    title: string;
    ui: {
      status_label: string;
      tone: Tone;
    };
    updated_at: string;
  }>;
  tasks_open: Array<{
    base: unknown;
    brief: string;
    closed_at: unknown;
    conversation: unknown;
    created_at: string;
    id: string;
    initiator: string;
    kind: string;
    repo: unknown;
    result: string;
    run_id: unknown;
    sandbox: unknown;
    session_id: unknown;
    source: string;
    status: string;
    thread: boolean;
    title: string;
    ui: {
      status_label: string;
      tone: Tone;
    };
    updated_at: string;
  }>;
  today: {
    cache_tokens: number;
    cost_usd: number;
    input_tokens: number;
    model_runs: number;
    output_tokens: number;
    runs: number;
  };
  tz: string;
  ui: {
    browser_watches: number;
    kept_files_total_text: string;
    memory_bytes: number;
    security_mode: string;
  };
  watch_now_daily: number;
  watches: Array<(Array<{
    baseline_at?: string;
    changes_in_row?: number;
    checked_at?: string;
    created_at?: string;
    cursor?: string;
    end_text?: string;
    exclude_words?: string;
    failures?: number;
    fetcher?: string;
    id?: string;
    include_words?: string;
    interval_minutes?: number;
    item_pattern?: string;
    last_error?: string | null;
    mode?: string;
    notify?: string;
    paused_reason?: string;
    run_id?: unknown;
    sent_count?: number;
    sent_day?: string;
    source?: string;
    start_text?: string;
    status?: string;
    ui?: {
      can_pause: boolean;
      can_resume: boolean;
      editable: boolean;
      interval_min: number;
      is_items: boolean;
      notify_label: string;
      status_label: string;
      tone: Tone;
      uses_browser: boolean;
      via_label: string;
    };
    updated_at?: string;
    url?: string;
    what?: string;
  }> | [{
    changes_in_row: number;
    created_at: string;
    end_text: string;
    exclude_words: string;
    failures: number;
    fetcher: string;
    id: string;
    include_words: string;
    interval_minutes: number;
    item_pattern: string;
    mode: string;
    notify: string;
    paused_reason: string;
    run_id: unknown;
    sent_count: number;
    sent_day: string;
    start_text: string;
    status: string;
    ui: {
      can_pause: boolean;
      can_resume: boolean;
      editable: boolean;
      interval_min: number;
      is_items: boolean;
      notify_label: string;
      status_label: string;
      tone: Tone;
      uses_browser: boolean;
      via_label: string;
    };
    updated_at: string;
    url: string;
    what: string;
  }, unknown])>;
  week: {
    cache_tokens: number;
    cost_usd: number;
    input_tokens: number;
    model_runs: number;
    output_tokens: number;
    runs: number;
  };
  weekly_cap: number | null;
}

/** 화면이 쓰는 상수와 이름표(`admin::derive::consts`). */
export interface Consts {
  activity_filters: Array<{
    key: string;
    label: string;
  }>;
  browser: {
    handover_keys: Array<{
      key: string;
      label: string;
    }>;
    handover_max_secs: number;
  };
  dev: {
    default_cost_cap_usd: number;
  };
  expressions: {
    keep_days: number;
    kinds: Array<{
      key: string;
      label: string;
      max_bytes: number;
      max_count: number;
    }>;
    max_categories: number;
  };
  gateway: {
    all_apps: string;
    app_role: string;
    roles: Array<{
      key: string;
      label: string;
    }>;
    scope_app_tools: string | null;
    scope_browser_handover: string | null;
    scope_owner_files: string | null;
    scope_security_mode: string | null;
    scopes: Array<{
      key: string;
      label: string;
    }>;
  };
  images: {
    default_daily_usd: number;
  };
  memory: {
    sections: Array<{
      key: string;
      label: string;
    }>;
  };
  pages: Array<{
    key: string;
    label: string;
    nav_label: string;
    path: string;
    tab_label: string;
  }>;
  profile: {
    avatar_rules: string;
    default_name: string;
    name_max_chars: number;
  };
  routines: {
    daily_wakes_max: number;
    daily_wakes_range: Array<number>;
  };
  security: {
    modes: Array<{
      icon: string;
      items: Array<string>;
      keeps: Array<string>;
      key: string;
      label: string;
      summary: string;
    }>;
  };
  settings_tabs: Array<string>;
  signals: {
    daily_range: Array<number>;
    poll_minutes_range: Array<number>;
  };
  watch: {
    browser_fetcher: string;
    fetchers: Array<{
      key: string;
      label: string;
    }>;
    host_gap_minutes: number;
    interval_max: number;
    interval_min: {
      items: number;
      region: number;
    };
    notify: Array<{
      key: string;
      label: string;
    }>;
    now_daily_range: Array<number>;
  };
  wrapup: {
    keep_days_range: Array<number>;
  };
}

/** `extra`: activity */
export interface ExtraActivity {
  filter: string;
}

/** `extra`: browser_handover */
export interface ExtraBrowserHandover {
  auto: boolean;
  available: boolean;
  left_secs: number | null;
  refresh: number | null;
  tab: string | null;
  tabs: Array<{
    id: string;
    title: string;
    url: string;
  }>;
  view: string;
}

/** `extra`: browser_sites */
export interface ExtraBrowserSites {
  view: string;
}

/** `extra`: character_edit */
export interface ExtraCharacterEdit {
  content: string | null;
  content_bytes: number | null;
  error: string | null;
  fingerprint: string | null;
  history: Array<{
    at: string;
    content: string;
    id: number;
    note: string;
    ui: {
      bytes: number;
    };
  }>;
  max_bytes: number;
  model: {
    change_note: string;
    choices: Array<{
      about: string;
      key: string;
      label: string;
    }>;
    config_default: string;
    label: string;
    picked: string | null;
  };
  pace: string;
  paces: Array<{
    key: string;
    label: string;
  }>;
  path: string | null;
  view: string;
}

/** `extra`: character_revision */
export interface ExtraCharacterRevision {
  is_current: boolean;
  revision: {
    at: string;
    content: string;
    id: number;
    note: string;
  };
  view: string;
}

/** `extra`: character_revision_missing */
export interface ExtraCharacterRevisionMissing {
  view: string;
}

/** `extra`: memory_entry */
export interface ExtraMemoryEntry {
  entry: {
    body: string;
    created_at: string;
    revision: number;
    section: string;
    slug: string;
    summary: string;
    title: string;
    ui: {
      section_key: string;
      section_label: string;
    };
    updated_at: string;
  };
  history: Array<{
    action: string;
    at: string;
    author: string;
    body: string;
    id: number;
    reason: string;
    run_id: string | null;
    section: string;
    slug: string;
    summary: string;
    title: string;
    ui: {
      author_label: string;
      deleted: boolean;
      origin: string;
    };
  }>;
  view: string;
}

/** `extra`: memory_list */
export interface ExtraMemoryList {
  query: string;
  view: string;
}

/** `extra`: memory_missing */
export interface ExtraMemoryMissing {
  view: string;
}

/** `extra`: skills_detail */
export interface ExtraSkillsDetail {
  revisions: Array<{
    at: string;
    author: string;
    body: string;
    reason: string;
    revision: number;
    script: string | null;
    ui: {
      author_label: string;
    };
  }>;
  skill: {
    created_at: string;
    description: string;
    footprint: {
      hosts: Array<string>;
      tools: Array<string>;
    };
    last_used_at: unknown;
    pinned: boolean;
    revision: number;
    script_tools: Array<string>;
    slug: string;
    source: string;
    status: string;
    successes: number;
    task_id: unknown;
    title: string;
    ui: {
      can_approve: boolean;
      can_restore: boolean;
      footprint: string;
      source_label: string;
      status_label: string;
      tone: Tone;
      usable: boolean;
    };
    updated_at: string;
    uses: number;
  };
  view: string;
}

/** `extra`: skills_list */
export interface ExtraSkillsList {
  skills: Array<{
    created_at: string;
    description: string;
    footprint: {
      hosts: Array<string>;
      tools: Array<string>;
    };
    last_used_at: unknown;
    pinned: boolean;
    revision: number;
    script_tools: Array<string> | null;
    slug: string;
    source: string;
    status: string;
    successes: number;
    task_id: string | null;
    title: string;
    ui: {
      can_approve: boolean;
      can_restore: boolean;
      footprint: string;
      source_label: string;
      status_label: string;
      tone: Tone;
      usable: boolean;
    };
    updated_at: string;
    uses: number;
  }>;
  view: string;
}
