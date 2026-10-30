// 由 plugin-api/generate-tools.mjs 自动生成，请勿手改。
// 工具箱包体：version 1793258457610（2026-10-29 生成）
// 工具箱更新后重新跑：node plugin-api/generate-tools.mjs

/** 每个 WS 工具的入参（来自工具箱自己的 JSON Schema）。 */
export interface CoconutToolParams {
  /**
   * 为当前队伍中的角色创建一条**自定义倍率**（自定义直伤）。固定值与百分比至少填一个（>0）；pctUnit 为 攻击% / 生命% / 防御% / 偏谐系数；element 为 物理/冷凝/热熔/导电/气动/衍射/湮灭。创建后可用 get_char_skills 看到它，再用 bind_damage_to_block 绑定到操作块。
   */
  add_custom_hit: {
    /** 角色名（须在当前队伍中） */
    character: string;
    /** 倍率名（同角色内唯一，例如「自定义重击」） */
    name: string;
    /** 固定值（默认 0；与 pctValue 至少一个 >0） */
    flatValue?: number;
    /** 百分比数值（默认 0；与 flatValue 至少一个 >0） */
    pctValue?: number;
    /** 百分比单位（默认 攻击%） */
    pctUnit?: "攻击%" | "生命%" | "防御%" | "偏谐系数";
    /** 伤害属性（默认 物理） */
    element?: "物理" | "冷凝" | "热熔" | "导电" | "气动" | "衍射" | "湮灭";
    /** 段数（默认 1，整数 1-999） */
    hits?: number;
  };

  /**
   * 在当前排轴指定轨道（1-3）追加一个操作块，位置为三行最右空白位置（按顺序排轴：新块总是落在所有操作块之后）。key 支持：普攻/重击/闪避/跳跃/共鸣技能/共鸣解放/声骸技能/谐度破坏，或 Q/E/R/F/T 等字母。desc 为描述文本（如“变奏入场”）。返回新块的**序号**（时间轴上的第几个操作块）。
   */
  add_op_block: {
    /** 轨道 1-3 */
    track: number;
    /** 按键名（普攻/重击/闪避/跳跃/共鸣技能/共鸣解放/声骸技能/谐度破坏 或 字母） */
    key: string;
    /** 描述（可空） */
    desc?: string;
  };

  /**
   * 在当前排轴最右空白位置添加参考线（按顺序排轴：参考线落在所有操作块之后），用于标记时间节点（如启动轴/循环轴）。返回新参考线的**序号**。
   */
  add_ref_line: Record<string, never>;

  /**
   * 给指定角色槽位（1-3）第 N 个声骸（1-5）追加一条副词条。label 取值：攻击/生命/防御/暴击率/暴击伤害/共鸣效率/治疗加成/攻击%/生命%/防御%（攻击% 等百分比词条）。不传 value 时使用中档默认值。
   */
  add_substat: {
    /** 角色槽位（1-3） */
    char: number;
    /** 声骸槽位（1-5） */
    slot: number;
    /** 副词条名称 */
    label: string;
    /** 可选，覆盖中档默认值（百分数词条填数值，如 8 表示 8%） */
    value?: number;
  };

  /**
   * 把某条声骸词条方案套用到当前配队里的该角色（替换其 5 个声骸词条）。不传 name 时套用标准14词条；该角色不在配队中会报错。
   */
  apply_substat_plan: {
    /** 角色名 */
    character: string;
    /** 方案名；省略则套用标准14词条 */
    name?: string;
    /** 可选：true 表示套用标准14词条 */
    standard?: boolean;
  };

  /**
   * 将指定工程归档（从侧边栏隐藏，可在设置-归档管理中恢复）。
   */
  archive_project: {
    /** 工程 id（用 list_projects 获取） */
    id: string;
  };

  /**
   * 把指定工程 Buff 配置绑定到指定伤害条目（该条目计算时生效）。两个参数都填**序号**：条目序号见 get_damage_entries，工程 Buff 配置序号见 get_buff_confs。
   */
  bind_buff_conf_to_entry: {
    /** 伤害条目序号（见 get_damage_entries 的「[NN]」） */
    entry: number;
    /** 工程 Buff 配置序号（见 get_buff_confs 的「[NN]」） */
    buffConf: number;
  };

  /**
   * 把伤害倍率绑定到指定**操作块或参考线**（targetType 选择）：hits 为 [{character, hitName, hits?}]，hitName 用 get_char_skills 查询到的命中名（含角色技能、声骸技能、自定义直伤）；hits 为该命中次数（默认 1）。整体覆盖该目标原有的直伤命中；可一次绑定多条。参考线编号与操作块编号各自从 1 开始，见 get_timeline_summary。
   */
  bind_damage_to_block: {
    /** 目标类型：block=操作块（默认）/ refLine=参考线 */
    targetType?: "block" | "refLine";
    /** 目标**序号**（targetType=block 时为操作块序号，refLine 时为参考线序号） */
    block: number;
    /** 要绑定到该操作块的伤害条目列表 */
    hits: {
      /** 伤害来源角色名 */
      character: string;
      /** 倍率名（用 get_skill_options 查询） */
      hitName: string;
      /** 段数（默认 1） */
      hits?: number;
    }[];
  };

  /**
   * 把非直伤绑定到指定**操作块或参考线**（可覆盖原有绑定）：entries 为 [{name, layers?, responders?}]，名称用 get_non_direct_options 获取。谐度破坏/震谐响应/骇破响应 可带 responders（触发角色名数组，响应必须有）；效应必须带 layers（1-上限）。
   */
  bind_non_direct_to_block: {
    /** 目标类型：block=操作块（默认）/ refLine=参考线 */
    targetType?: "block" | "refLine";
    /** 目标序号（操作块或参考线） */
    block: number;
    /** 非直伤条目列表（整体覆盖该块）：处决/响应/效应 */
    entries: {
      /** 条目名（用 get_non_direct_options 查询） */
      name: string;
      /** 层数（效应类用，默认 1） */
      layers?: number;
      /** 响应者角色名列表 */
      responders?: string[];
    }[];
  };

  /**
   * 让服务端向上游确认一次库街区登录是否仍有效（即设置里的「检验登录有效性」），返回有效性、原因、账号与绑定角色。同步报登录相关错误时用它确认。
   */
  check_kuro_login: Record<string, never>;

  /**
   * 清空 AI 上下文的分段**快照**（下一次请求前上下文面板显示空态）。用于排障或强制下一轮重建上下文快照；不会清空对话历史（对话历史由界面上的「清空对话历史」按钮处理），也不动用量的会话累计。
   */
  clear_ai_context_snapshot: Record<string, never>;

  /**
   * 清空整个本地 Buff 集（所有实体与 Buff，不可恢复）。
   */
  clear_buff_set: Record<string, never>;

  /**
   * 克隆指定工程（全部环节）为新工程，新工程名可指定。
   */
  clone_project: {
    /** 要克隆的源工程 id */
    id: string;
    /** 新工程名称 */
    newName: string;
  };

  /**
   * 创建一条空的工程 Buff 配置。返回新配置的**序号**（清单里的第几条，见 get_buff_confs），后续操作用这个序号。
   */
  create_buff_conf: {
    /** 工程 Buff 配置名称 */
    name: string;
  };

  /**
   * 新建一个工程并切换为当前活动工程。
   */
  create_project: {
    /** 工程名称 */
    name: string;
  };

  /**
   * 创建一张新的**自定义技能卡**（AI 可自创并自行管理）。name 必须与现有技能不重名；body 是激活后注入的完整操作规范/正文。主动技能需 AI 判断匹配后调用 use_skill 激活；被动技能每轮自动注入正文。创建后可用 update_skill 继续维护。
   */
  create_skill: {
    /** 技能名（唯一，例如「XX 角色配装口径」） */
    name: string;
    /** 一句话描述（主动技能清单会用它判断何时激活；建议不超过 60 字） */
    description?: string;
    /** 技能正文：激活后要遵守的完整规范/步骤/注意事项 */
    body: string;
    /** active=按需激活（默认）；passive=每轮常驻注入正文 */
    mode?: "active" | "passive";
    /** 是否启用（默认 true） */
    enabled?: boolean;
  };

  /**
   * 删除指定工程 Buff 配置（按**序号**，同时清理它对所有条目的绑定）。
   */
  delete_buff_conf: {
    /** 工程 Buff 配置序号 */
    buffConf: number;
  };

  /**
   * 从本地 Buff 集删除指定实体（不可恢复）。
   */
  delete_buff_set_entity: {
    /** 实体类型：character/weapon/echo/1set-5set */
    entityType: string;
    /** 要删除的实体名称（中文） */
    entityName: string;
  };

  /**
   * 永久删除指定工程（不可恢复）。
   */
  delete_project: {
    /** 要永久删除的工程 id */
    id: string;
  };

  /**
   * 删除一张**自定义**技能卡（按名称定位）。内置技能不可删除，只能通过 update_skill 禁用。删除后使用 use_skill 将无法再激活它。
   */
  delete_skill: {
    /** 要删除的技能名 */
    name: string;
  };

  /**
   * 删除一条自定义声骸词条方案（不可恢复；标准14词条不能删除，需要清掉改动请用 reset_standard_substat_plan）。
   */
  delete_substat_plan: {
    /** 角色名 */
    character: string;
    /** 方案名 */
    name: string;
  };

  /**
   * 禁用某条参考线的时间记点（按**序号**，从 timings 移除，不再参与 DPS 分段）。若该参考线未启用则幂等返回。
   */
  disable_ref_line_timing: {
    /** 参考线序号 */
    line: number;
  };

  /**
   * 复制指定工程 Buff 配置为一条新配置（按**序号**），可指定新名称（默认“原名 复制”）。返回新配置的序号。
   */
  duplicate_buff_conf: {
    /** 要复制的 工程 Buff 配置序号 */
    buffConf: number;
    /** 新集名称（可空） */
    customName?: string;
  };

  /**
   * 启用某条参考线作为时间记点（按**序号**）。启用时秒数自动推导：按参考线命名解析时间片段（如 "1m30s"→90、"起手"→null 未解析）；结束线（最后一条）默认 120s。若该参考线已启用则幂等返回当前状态。
   */
  enable_ref_line_timing: {
    /** 参考线序号（见 get_timeline_summary 的「[线N]」） */
    line: number;
  };

  /**
   * 把当前配队中某角色**正在使用的 5 个声骸与副词条**快照成一条自定义词条方案（用于「先存快照 → 改配装/套用其它方案 → 对比 → 回滚」）。默认生成不重名的时间戳方案名；显式传 name 且已存在同名方案时必须传 overwrite=true 才会覆盖。回滚用 apply_substat_plan 套用该快照。
   */
  export_substat_plan: {
    /** 角色名（须在当前配队中） */
    character: string;
    /** 可选：快照方案名；省略则自动生成时间戳名 */
    name?: string;
    /** 可选：同名方案已存在时是否覆盖（默认 false） */
    overwrite?: boolean;
  };

  /**
   * 自动格式化排轴：各操作块右边界对齐下一个块（可跨角色）的左边界，参考线跟随其左右块。
   */
  format_timeline: Record<string, never>;

  /**
   * 为本地 Buff 集中的指定实体（character/weapon/echo/1set-5set）生成 Buff 并写入主页的本地 Buff 集；仅在用户明确要求维护 Buff 集时使用，不用于给工程配 Buff（整体覆写该实体，来源变为自定义）。该工具会自动查询实体官方详情（角色技能/共鸣链/武器效果等）并提取 Buff，无需先调用其它查询工具；生成前若未定义命名规则会先询问用户。
   */
  generate_buff_set_entity_buffs: {
    /** 实体类型 */
    entityType: string;
    /** 实体名称（中文） */
    entityName: string;
  };

  /**
   * 为当前工程队伍中的实体（角色/武器/首位声骸/触发套装）逐个生成工程 Buff 配置并导入当前工程拉表（含归属绑定）。该工具会自动查询各实体官方详情并提取 Buff，无需先调用其它查询工具。默认遍历全队，可用 slot（1-3）或 entityType 过滤。生成前若未定义命名规则会先询问用户。
   */
  generate_project_buff_confs: {
    /** 可选：只处理该槽位（1-3） */
    slot?: number;
    /** 可选：只处理该实体类型 */
    entityType?: string;
  };

  /**
   * 读取 AI 助手的**上下文管理器**状态：本轮装配进请求的各分段（人设 / 被动技能 / 主动技能 / 工程上下文 / 变更队列 / 历史 / 用户输入）的字符数、估算 token 与占用比例，以及哪些分段被临时禁用。首轮请求前快照为空（hasSnapshot=false）。返回的 id 可直接传给 set_ai_context_segment 临时禁用或恢复。
   */
  get_ai_context_state: Record<string, never>;

  /**
   * 读取 AI 配置文件列表（每个含服务地址/模型/思考强度，apiKey 仅返回是否已设置不暴露明文）及当前激活的配置 id。
   */
  get_ai_profiles: Record<string, never>;

  /**
   * 读取 AI 助手的**实时运行情况**：是否正在请求、当前阶段（思考中 / 收到流式文本 / 正在调用工具 / 等待工具结果 / 空闲）、已耗时、当前工具名与补充说明、长任务模式状态（是否开启 / 目标 / 已自动继续次数），以及本轮已发生的工具调用列表（名称 / 参数 / 耗时 / 成败 / 结果梗概）。用于自检卡在哪一步或复盘本轮调用了哪些工具。
   */
  get_ai_turn_state: Record<string, never>;

  /**
   * 读取 AI 助手的 **token 与缓存命中用量**：本轮（turn）合计、本会话（session）累计，含 prompt / completion / total token、缓存命中与未命中 token、缓存命中率、请求数与其中「无服务商 usage、走本地估算回退」的请求数。字段缺失时返回 null（界面显示「—」，绝不臆造），estimatedRequests>0 说明有请求是本地估算值，不能当计费依据。
   */
  get_ai_usage: Record<string, never>;

  /**
   * 获取指定工程 Buff 配置的完整详情：作用范围、是否全局、生效条件（链/阶硬门槛 + 属性/类型条件）、每个乘区条目（zoneId/数值/是否覆盖/引用/**各自的乘区级条件**）及其生效角色槽位。同一乘区可有多条，每条各自判定条件后相加；覆盖唯一（同一乘区仅一个覆盖条目）。
   */
  get_buff_conf_detail: {
    /** 工程 Buff 配置序号（见 get_buff_confs 的「[NN]」） */
    buffConf: number;
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 读取某条工程 Buff 配置内指定乘区的**乘区级生效条件**（伤害属性 / 伤害类型；类内「或」）。只读取，不修改。同一乘区有多条贡献条目时返回第一条的条件，完整逐条清单请用 get_buff_conf_detail。
   */
  get_buff_conf_zone_condition: {
    /** 工程 Buff 配置序号（见 get_buff_confs 的「[NN]」） */
    buffConf: number;
    /** 乘区 id（接受旧 id，会自动重映射） */
    zoneId: string;
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 获取工程 Buff 配置清单（一行一条）：行首方括号里的数字就是**序号**（create/rename/delete/绑定/乘区工具都填它）、名称、作用范围（self/self_except/team/effect_only/all）、是否全局默认、生效条件、乘区条数。**不含各乘区的数值/引用**——那部分用 get_buff_conf_detail 按需查；某条目绑了哪些 Buff 见 get_damage_entries。可选 zoneId：只列出含该乘区的配置，并在每项附带命中乘区的数值/引用摘要；不传则保持原来的全量清单。
   */
  get_buff_confs: {
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
    /** 可选：乘区 id（如 atkPct / critRate，旧 id 会自动重映射）；只返回含该乘区的配置 */
    zoneId?: string;
    /** 输出格式：text（默认）或 json（结构化 items，字段含 no/id/scope/zoneCount 等） */
    format?: "text" | "json";
  };

  /**
   * 查看本地 Buff 集中指定实体的全部 Buff 详情（不读取工程 Buff 配置；名称、作用范围、生效条件、乘区与数值、引用）。
   */
  get_buff_set_entity_buffs: {
    /** 实体类型：character/weapon/echo/1set-5set */
    entityType: string;
    /** 实体名称（中文，用 list_buff_set_entities 定位） */
    entityName: string;
  };

  /**
   * 获取主页的本地 Buff 集概览（不读取工程 Buff 配置）：实体数量、按类型分布、数据来源（工坊同步/自定义）。
   */
  get_buff_set_summary: Record<string, never>;

  /**
   * 读取各类缓存条目数（列表/详情/图像）。可用 set_setting key=clear_cache 清理（值 list/info/image/all）。
   */
  get_cache_counts: Record<string, never>;

  /**
   * 获取指定角色可绑定的伤害命中列表（技能类型、命中名、倍率、元素），含：角色技能、装备声骸技能、用户自定义直伤（技能类型分别为声骸技能/自定义）。用于把伤害倍率绑定到操作块。**可跨工程只读**：传 projectId 时，声骸技能与自定义倍率按该工程队伍/数据读取，不切换活动工程。
   */
  get_char_skills: {
    /** 角色名 */
    character: string;
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 获取链/阶配置（每个角色的共鸣链 0-6 与武器精炼 0-5，0=未精炼）及“可用Buff”过滤开关状态。**可跨工程只读**：传 projectId 读任意工程且不切换当前工程；省略则读活动工程。
   */
  get_condition_profile: {
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 获取配装配置摘要：每个角色的 5 个声骸（cost、主词条、副词条）与敌人配置（类型/等级/防御/减伤/各抗性）。**可跨工程只读**：传 projectId 读任意工程且不切换当前工程；省略则读活动工程。
   */
  get_config_summary: {
    /** 可选：工程 id（用 list_projects 获取）；省略 = 当前活动工程 */
    projectId?: string;
  };

  /**
   * 列出当前排轴上每个**操作块 / 参考线**绑定的伤害：targetType、目标序号、伤害块 id、直伤命中（1 起的 hitIndex、角色、技能类型、命中名、倍率、属性、段数）与非直伤条目。用于定位要解绑/修改的具体一条命中。
   */
  get_damage_bindings: Record<string, never>;

  /**
   * 获取当前工程的伤害条目（拉表）：**按条目在时间轴上的顺序**逐条给出「**序号** / 归属角色 / 显示名 / **倍率** / 属性 / 伤害类型 / 已绑 Buff 名 / **稳定条目 id**」。行首方括号里的数字就是序号，增删改（bind_buff_conf_to_entry / set_entry_damage_types / toggle_damage_type / rename_damage_entry / get_damage_entry_buff_sources）填它。行尾 id 与 get_result_summary 的 entryId 同源，可用来在「伤害条目 ↔
   */
  get_damage_entries: {
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
    /** 输出格式：text（默认）或 json（结构化 items，字段含 no/id/ratioValue 等） */
    format?: "text" | "json";
  };

  /**
   * 查询某伤害条目的**跨角色影响源**（按**序号**）：本段引用了其它角色的面板（乘区 ref 里 characterIdx 指向他角色）时，作用域指向那个角色、且会改写被引用面板乘区的 Buff —— 这些 Buff 必须用 bind_buff_conf_to_entry 勾到本段才会参与该角色在这一段的面板计算（拉表里它们以「影响源」列/勾选项出现，不是自动生效的）。返回每条影响源的**工程 Buff 配置序号**与名称、被引用角色槽位、被改写的面板乘区。
   */
  get_damage_entry_buff_sources: {
    /** 伤害条目序号（见 get_damage_entries 的「[NN]」） */
    entry: number;
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 查询结果页「数据分析」弹窗的全部分析结果：全队总伤害与 DPS、队伍伤害占比（各角色）、各角色直伤类型占比（普攻/重击/共技/共解等）、三种词条贡献算法（single-loss 单减损 / shapley 夏普利 / partial-derivative 偏导）的副词条贡献明细，每个角色还带**表级14词分**（standardSubstatPctNorm：把该角色换成「标准 14 词条」方案后的同样分数，可与总分对照），以及**每一条 Buff 对角色直伤的贡献**（按「单独移除该 Buff 后损失多少伤害」衡量：逐条损失 + 每条的逐乘区损失 + 移除全部 Buff 的总损失；逐条损失不可
   */
  get_data_analysis: {
    /** 可选：工程 id；传了则只读分析该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 查询实体的官方游戏数据详情：角色（技能/共鸣链（俗称命座）/固有属性）、武器（效果）、声骸（技能）、套装（各件数加成）。用于向用户说明实体机制、核对生成内容，或判断该实体适合生成哪些 Buff。
   */
  get_entity_info: {
    /** 实体类型 */
    entityType: string;
    /** 实体名称（中文，用 list_entities / search_entities 定位） */
    entityName: string;
  };

  /**
   * 读取按键图标映射（每个操作动作 → 显示的键盘/鼠标图标 key）。返回所有按键映射条目。
   */
  get_keymap: Record<string, never>;

  /**
   * 查看库街区登录态与声骸数据暂存：是否登录、账号、绑定角色、登录有效性、本地暂存的数据时间与可刷新倒计时。判断「要不要刷新数据」「是否还登录着」时先调它。
   */
  get_kuro_state: Record<string, never>;

  /**
   * 获取可绑定到操作块的非直伤选项：谐度破坏（处决，可带触发角色）、震谐响应/骇破响应（偏谐响应，必须带触发角色）、各类效应（层数 1-上限、元素），以及电磁爆发（须先绑电磁效应）。
   */
  get_non_direct_options: Record<string, never>;

  /**
   * 查看当前所有弹窗面板的开关状态（工程 Buff 配置/速查/主页 Buff 集/设置/工坊/角色详情配置/从 Buff 集导入工程 Buff 配置等）。
   */
  get_panels_state: Record<string, never>;

  /**
   * 获取工程状态：工程名、队伍（各槽位角色与武器）、各环节锁定情况。**可跨工程只读**：传 projectId 读任意工程且不切换当前工程；省略则读活动工程。AI 动手前应调用以了解现状。要看某一个环节的**内容**，用按时间顺序渲染的摘要工具（比原始工程 JSON 紧凑）：排轴 get_timeline_summary（也支持 projectId）、拉表 get_damage_entries、配装 get_config_summary。
   */
  get_project_state: {
    /** 可选：工程 id（用 list_projects 获取）；省略 = 当前活动工程 */
    projectId?: string;
  };

  /**
   * 获取某角色的推荐武器（官方推荐顺序，首项通常为最优）。为队伍角色挑武器、或需要了解某角色常用武器时调用；返回的武器名可直接传给 set_team_weapon。
   */
  get_recommended_weapons: {
    /** 角色名称（中文，可用 get_team_catalog 查询） */
    characterName: string;
  };

  /**
   * 查询时间参考线记点状态：哪些参考线已启用为时间记点、各自的秒数（null=未填写/未解析，不参与 DPS 分段）。秒数来源：自动推导（从参考线命名解析）或自定义覆盖。结果按参考线在时间轴上的位置排序，并给出**序号**。**可跨工程只读**：传 projectId 读任意工程且不切换当前工程。
   */
  get_ref_line_timings: {
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 查询单条伤害结果条目及其全部乘区溯源（每段的数值与来源：基础值/白值/绿值、增伤/加深/易伤/抗性/防御/免伤/集谐/谐度/同奏/终伤/特殊/暴击等乘区，各乘区的 buff 来源与折算贡献。集谐区只有直伤吃，谐度区只有处决/偏谐响应吃，两者是两个独立乘区）。同时给出倍率因子拆解（factors）：复合倍率（多成分/多段乘数）与额外倍率 buff 各自分摊到的倍率、占比与该因子的**单段**伤害（成分乘数与条目段数已归一到 1 段）；factor.damage × (factor.weight / factor.segmentRatio) 逐因子求和 = 条目期望。entryId 用 get_r
   */
  get_result_entry_breakdown: {
    /** 伤害条目 id（get_result_summary 获取） */
    entryId: string;
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 基于当前配装/Buff/条件配置计算并返回伤害结果摘要：每个伤害条目的期望伤害（含暴击）与全队总伤害。支持 summaryOnly 只取总计、groupBy:"character" 按角色汇总（带占比）、character 只统计单个角色，减少逐条目输出。可用来回答“这套配置伤害多少”“哪个技能伤害最高”“各角色伤害占比”。
   */
  get_result_summary: {
    /** 可选：工程 id；传了则只读计算该工程，不切换活动工程 */
    projectId?: string;
    /** 只返回总计（以及 groupBy=character 时的角色汇总），省略逐条目明细 */
    summaryOnly?: boolean;
    /** 聚合方式：entry（默认，返回逐条目）或 character（按角色汇总） */
    groupBy?: "entry" | "character";
    /** 可选：只统计该角色（按显示名精确匹配） */
    character?: string;
  };

  /**
   * 读取当前设置状态——覆盖「设置」弹窗全部可配置项：外观主题、按键图标、交互（含锁定水印）、性能、工坊、连接配置（数据源）、缓存、助手设置。具体子项可用专用工具查询（get_keymap/get_shortcuts/get_ai_profiles/get_cache_counts）。
   */
  get_settings_state: Record<string, never>;

  /**
   * 读取工程「分享」的 10 分钟频率限制状态：是否处于冷却中、剩余毫秒与 mm:ss 文案、冷却时长常量。分享本身是上传+本地导出动作，需用户在工程侧边栏右键发起（AI/WS 不代发），本工具只用于告知用户还要等多久。
   */
  get_share_cooldown: Record<string, never>;

  /**
   * 读取界面快捷键映射（排轴/拉表各操作的快捷键）。返回所有快捷键定义及其当前绑定键。
   */
  get_shortcuts: Record<string, never>;

  /**
   * 读取一张技能卡的完整内容（名称、类型、描述、正文、是否启用、是否内置）。修改前先读；内置技能「Buff 命名规则」「黑话词典」等也可通过它读取正文。
   */
  get_skill: {
    /** 技能名（可用 list_skills 查询，需完全一致） */
    name: string;
  };

  /**
   * 查看某个声骸词条方案的完整 5 槽位明细（cost、主词条、第二主词条、副词条）。不传 name 时返回该角色的标准14词条（自动生成/工坊/本地修改都算）。
   */
  get_substat_plan: {
    /** 角色名 */
    character: string;
    /** 方案名；省略则取标准14词条 */
    name?: string;
    /** true 表示取该角色的标准14词条 */
    standard?: boolean;
  };

  /**
   * 读取拉表（表格）撤销/重做历史状态：是否可撤销、是否可重做。拉表历史与排轴历史相互独立——本工具只看表格，排轴用 get_timeline_summary 配合 undo_timeline/redo_timeline。
   */
  get_table_history: Record<string, never>;

  /**
   * 获取队伍配置摘要（每个槽位的角色、武器、触发套装、声骸）。**可跨工程只读**：传 projectId 读任意工程且不切换当前工程；省略则读活动工程。
   */
  get_team: {
    /** 可选：工程 id（用 list_projects 获取）；省略 = 当前活动工程 */
    projectId?: string;
  };

  /**
   * 获取可用的队伍配置数据：角色（元素/武器类型）、武器（类型/星级）、声骸（cost/所属套装）、套装（支持件数）。设置队伍前先调用以获取准确名称。
   */
  get_team_catalog: Record<string, never>;

  /**
   * **按需**查询排轴里每个已绑定伤害的**倍率明细**（命中名、倍率、属性、系数类型；含效应/处决/响应的折算结果），按排轴顺序编号，每行一条；序号与排轴位置不代表秒数。只想看排轴结构（哪些块、绑了什么）请用 get_timeline_summary —— 那里不含倍率，避免无谓的 token 开销。**可跨工程只读**：传 projectId 读任意工程且不切换当前工程。
   */
  get_timeline_damage_list: {
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
    /** 输出格式：text（默认）或 json（结构化 items，每项一处已绑定倍率） */
    format?: "text" | "json";
  };

  /**
   * 获取排轴：**按排轴顺序**列出所有操作块、参考线与已绑定的伤害。每行给出序号、轨道与角色、操作内容，其下缩进列出该块绑定的伤害命中（只给命中名，**不含倍率**——倍率用 get_timeline_damage_list 按需查）。排轴位置只是定位坐标，不代表秒数；参考线时间优先取数据分析配置的有效记点，没有有效记点时按“时间记点自动配置”规则从名称推导，并标为自动推导。**可跨工程只读**：传 projectId 读任意工程的持久化排轴且不切换当前工程（结构与绑定名；倍率明细用 get_timeline_damage_list 同样传 projectId 查）；省略则读活动工程。
   */
  get_timeline_summary: {
    /** 可选：读取指定工程（用 list_projects 获取），不切换当前工程；省略 = 当前活动工程 */
    projectId?: string;
    /** 输出格式：text（默认，便于阅读）或 json（结构化 items，便于解析） */
    format?: "text" | "json";
  };

  /**
   * 把主页 Buff 集中指定实体（角色/武器/首位声骸/套装）的全部 Buff 导入当前工程 Buff 配置（保留实体归属，导入后可再绑定到伤害条目）。entityType 取值：character/weapon/echo/1set/2set/3set/4set/5set。
   */
  import_buff_set_entity_to_project: {
    /** 实体类型 */
    entityType: string;
    /** 实体名称 */
    entityName: string;
  };

  /**
   * 退出库街区登录（同时清除本地声骸数据暂存）。用户要求退出或换账号时使用。
   */
  kuro_logout: Record<string, never>;

  /**
   * 列出本地 Buff 集的实体（仅管理主页数据，不修改工程 Buff 配置；可按类型过滤）：实体名、类型、来源（share/custom）、Buff 数量。
   */
  list_buff_set_entities: {
    /** 可选：character/weapon/echo/1set-5set */
    entityType?: string;
  };

  /**
   * 列出自定义倍率（自定义直伤）：按角色返回名称、固定值、百分比、单位、属性、段数与渲染后的倍率串。character 省略时列出当前队伍全部角色。创建 / 修改后用 get_char_skills 也能看到它们并绑定到操作块。
   */
  list_custom_hits: {
    /** 可选：只列该角色的自定义倍率 */
    character?: string;
    /** 可选：工程 id；传了则只读该工程，不切换活动工程 */
    projectId?: string;
  };

  /**
   * 列出某类游戏实体的全部名称（character=角色 / weapon=武器 / echo=声骸 / 1set-5set=声骸套装件数）。用于定位实体名，供 get_entity_info 查询详情或生成 Buff。
   */
  list_entities: {
    /** 实体类型 */
    entityType: string;
  };

  /**
   * 列出所有本地工程（含名称、是否当前活动、是否已归档）。AI 需要了解有哪些工程时调用。
   */
  list_projects: Record<string, never>;

  /**
   * 列出用户的技能卡（名称 + 类型 + 一句话描述 + 是否启用 + 是否内置）。主动技能在任务匹配时用 use_skill 激活其正文；被动技能已常驻生效，正文已在系统提示中，无需激活。要读取正文用 get_skill。
   */
  list_skills: Record<string, never>;

  /**
   * 列出词条集（快速词条方案）里的声骸词条方案。传 character 时列出该角色全部方案（第一项固定为「标准14词条」，其余为自定义）；不传时列出本地库里有方案的角色，并附带每个角色的标准方案来源、自定义方案名、cost 与副词条总数，便于直接挑选方案而不必逐角色再查。
   */
  list_substat_plans: {
    /** 可选：角色名（如「绯雪」） */
    character?: string;
  };

  /**
   * 锁定当前活动工程的指定环节（team/timeline/calculation/config）。
   */
  lock_phase: {
    /** 要锁定的环节（锁后其内容不可修改） */
    phase: string;
  };

  /**
   * 管理 AI 配置文件：add=新建（传 label，可选 baseUrl/model/apiKey/reasoningEffort/contextWindow/maxOutputTokens/maxToolRounds/directRequest）、switch=切换激活（传 id）、update=修改（传 id + 可选字段）、delete=删除（传 id，至少保留 1 个）。reasoningEffort 须为 low/medium/high；directRequest=true 表示由浏览器直连（绕过站点转发）。返回操作结果（apiKey 永远不回显明文）。
   */
  manage_ai_profile: {
    /** 操作类型 */
    action: "add" | "switch" | "update" | "delete";
    /** 配置 id（switch/update/delete 用） */
    id?: string;
    /** 配置名称（add 用，update 可选） */
    label?: string;
    /** 服务地址（add/update 可选，如 https://api.deepseek.com） */
    baseUrl?: string;
    /** 模型名（add/update 可选） */
    model?: string;
    /** API Key（add/update 可选；空串=清除） */
    apiKey?: string;
    /** 思考强度（add/update 可选） */
    reasoningEffort?: "low" | "medium" | "high";
    /** 上下文窗口 token 数（add/update 可选；用量圈分母，默认 256000） */
    contextWindow?: number;
    /** 单次请求最大输出 token（add/update 可选；默认 65536，太小会导致回复截断） */
    maxOutputTokens?: number;
    /** 单回合工具轮次上限（add/update 可选；默认 48） */
    maxToolRounds?: number;
    /** 是否浏览器直连（add/update 可选；true=绕过站点转发，需要端点支持 CORS） */
    directRequest?: boolean;
  };

  /**
   * 管理工坊实例：switch=切换到指定实例（传 id）、add=添加实例（传 url）、remove=删除实例（传 id，至少保留 1 个）、reset=恢复默认实例列表。返回当前实例列表与选中 id。
   */
  manage_workshop: {
    /** 操作类型 */
    action: "switch" | "add" | "remove" | "reset";
    /** 实例 id（switch/remove 用） */
    id?: string;
    /** 实例地址（add 用） */
    url?: string;
  };

  /**
   * 把已有操作块移动到指定位置（按**序号**）：position 为 {time: 秒}（绝对时间 0 至当前结束线）或 {anchor: 块序号, side: before/after（默认 after）, offset?: 秒}（相对某块）。移动后自动消除同轨道重叠。
   */
  move_op_block: {
    /** 要移动的操作块序号 */
    block: number;
    /** 把操作块移动到哪个时间位置：{time: 秒}（绝对时间 0 至当前结束线）或 {anchor: 块序号, side, offset}（相对某块） */
    position: {
      /** 绝对时间（秒，0 至当前结束线） */
      time?: number;
      /** 目标操作块序号（相对它移动） */
      anchor?: number;
      side?: "before" | "after";
      /** 相对偏移（秒，默认 0） */
      offset?: number;
    };
  };

  /**
   * 把已有参考线移动到指定位置（按**序号**）：position 为 {time: 秒}（绝对时间 0 至当前结束线）或 {anchor: 块序号, side: before/after, offset?: 秒}（相对某块）。与相邻参考线保持最小间距，过近会报错。
   */
  move_ref_line: {
    /** 参考线序号（见 get_timeline_summary 的「[线N]」） */
    line: number;
    /** 把参考线移动到哪个时间位置：{time: 秒}（绝对时间 0 至当前结束线）或 {anchor: 块序号, side, offset}（相对某块） */
    position: {
      /** 绝对时间（秒，0 至当前结束线） */
      time?: number;
      /** 目标操作块序号（相对它移动） */
      anchor?: number;
      side?: "before" | "after";
      /** 相对偏移（秒，默认 0） */
      offset?: number;
    };
  };

  /**
   * 打开「词条集 → 从库街区同步」预览弹窗（会先打开词条集面板，然后载入当前暂存/上游数据），由用户在卡片列表里勾选要写入的角色。需要用户自己挑角色或改方案名时用它，而不是直接写入。
   */
  open_kuro_sync_preview: Record<string, never>;

  /**
   * 打开或关闭指定弹窗面板：buff-set=主页 Buff 集（打开时先返回主页）；buff-conf=当前工程拉表的工程 Buff 配置；buff-conf-import=从 Buff 集导入工程 Buff 配置。两者是不同入口，用户说“打开 Buff 集”时用 buff-set，含义不清时先用 ask_user 确认。其它 panel 取 get_panels_state 返回的 name（quick-lookup/substat-library/settings/workshop/character-detail/damage-list 等）；open 默认 true。
   */
  open_panel: {
    /** buff-set=主页 Buff 集；buff-conf=工程 Buff 配置；buff-conf-import=导入工程 Buff 配置；其它见 get_panels_state */
    panel: string;
    /** 打开(true)/关闭(false)，默认 true */
    open?: boolean;
  };

  /**
   * 重做上一次被撤销的**拉表（表格）**变更。只影响表格，不动排轴与词条；排轴请用 redo_timeline。没有可重做的操作时 ok=false 并给出原因。
   */
  redo_table: Record<string, never>;

  /**
   * 重做上一次撤销的排轴操作。
   */
  redo_timeline: Record<string, never>;

  /**
   * 重新排布指定轨道（1-3）的操作块，消除重叠。
   */
  reflow_track: {
    /** 要重排的轨道（1-3） */
    track: number;
  };

  /**
   * 强制从库街区重新拉取当前绑定角色的声骸数据并写入本地暂存（同一份数据 5 分钟只能刷新一次，冷却期内会报错）。平时同步直接用本地暂存、不打上游；只有用户明确要「最新数据」时才调用。
   */
  refresh_kuro_echo_data: Record<string, never>;

  /**
   * 从工程 Buff 配置中删除指定乘区（不可恢复；同一乘区的多条贡献条目会全部删除）。乘区 id 接受旧 id（customFinalDmg/customFinalDmgMul 会自动重映射）。
   */
  remove_buff_conf_zone: {
    /** 工程 Buff 配置序号 */
    buffConf: number;
    /** 乘区 id（接受旧 id，会自动重映射） */
    zoneId: string;
  };

  /**
   * 删除一条自定义倍率（按角色 + 倍率名定位）。已经绑定到操作块的同名命中不会自动解除绑定，删除前请确认不再需要。
   */
  remove_custom_hit: {
    /** 角色名（须在当前队伍中） */
    character: string;
    /** 要删除的自定义倍率名 */
    hitName: string;
  };

  /**
   * 清空指定操作块 / 参考线上的**全部伤害绑定**（直伤 + 非直伤），但保留操作块 / 参考线本身。等价于「删除该目标的伤害条目」。
   */
  remove_damage_from_target: {
    /** 目标类型（默认 block） */
    targetType?: "block" | "refLine";
    /** 目标序号（操作块或参考线） */
    block: number;
  };

  /**
   * 删除指定操作块（按**序号**）。删除后其余块的序号会前移，后续操作请重新读 get_timeline_summary。
   */
  remove_op_block: {
    /** 操作块序号（见 get_timeline_summary 的「[块N]」） */
    block: number;
  };

  /**
   * 删除指定参考线（按**序号**）。删除后其余参考线的序号会前移，后续操作请重新读 get_timeline_summary。
   */
  remove_ref_line: {
    /** 参考线序号 */
    line: number;
  };

  /**
   * 移除指定角色槽位（1-3）第 N 个声骸（1-5）的第 idx 条副词条（从 0 开始）。
   */
  remove_substat: {
    /** 角色槽位（1-3） */
    char: number;
    /** 声骸槽位（1-5） */
    slot: number;
    /** 副词条下标（从 0 开始） */
    idx: number;
  };

  /**
   * 重命名指定工程 Buff 配置（按**序号**）。
   */
  rename_buff_conf: {
    /** 工程 Buff 配置序号（见 get_buff_confs 的「[NN]」） */
    buffConf: number;
    /** 新名称 */
    name: string;
  };

  /**
   * 修改伤害条目的显示名（按 get_damage_entries 的**序号**）。只改 AI/界面看到的 displayName，不改底层命中 id、倍率或 Buff 绑定；传空串恢复默认显示名。
   */
  rename_damage_entry: {
    /** 伤害条目序号（见 get_damage_entries 的「[NN]」） */
    entry: number;
    /** 新显示名（空串=恢复默认） */
    name: string;
  };

  /**
   * 重命名指定工程。
   */
  rename_project: {
    /** 工程 id（用 list_projects 获取）；空字符串表示返回主页 */
    id: string;
    /** 新名称 */
    name: string;
  };

  /**
   * 重命名一条自定义方案（标准14词条不能改名）。
   */
  rename_substat_plan: {
    /** 角色名 */
    character: string;
    /** 当前方案名 */
    name: string;
    /** 新方案名 */
    newName: string;
  };

  /**
   * 恢复全部被临时禁用的 AI 上下文分段（把所有分段重新纳入后续请求）。只影响当前会话，不动持久化配置。
   */
  reset_ai_context_segments: Record<string, never>;

  /**
   * 重置某角色的标准14词条：清掉本地修改/工坊同步的版本，回落到「工坊同步」或按角色数据自动生成。
   */
  reset_standard_substat_plan: {
    /** 角色名 */
    character: string;
  };

  run_js: {
    /** 要执行的脚本体。例如：`const t = await get_team({}); return t.members.length` */
    code: string;
  };

  /**
   * 新建或覆盖一条声骸词条方案（覆盖会先弹确认框）。standard=true 时写的是该角色的标准14词条，要求副词条恰好 14 条；自定义方案要求 5 个槽位、cost 取值 1/3/4 且合计 ≤12、每槽副词条 ≤5 且不重复。slots 里主词条可写字符串（如 "暴击率"，数值自动取满级），副词条可写 "暴击率" 或 {type:"暴击率",value:7.5}（数值自动吸附到合法档位）。
   */
  save_substat_plan: {
    /** 角色名 */
    character: string;
    /** 方案名（standard=true 时忽略，固定为「标准14词条」） */
    name?: string;
    /** 可选：true 表示写标准14词条 */
    standard?: boolean;
    /** 5 个声骸槽位 */
    slots: {
      /** cost：1/3/4 */
      cost: number;
      /** 主词条名称，空字符串表示不选 */
      mainStat?: string;
      /** 该声骸的副词条 */
      substats?: string[];
    }[];
  };

  /**
   * 按关键词模糊搜索游戏实体名称（角色/武器/声骸/套装任意类型），用于定位准确名称后再查详情。
   */
  search_entities: {
    /** 搜索关键词（中文片段） */
    query: string;
    /** 可选：只搜该类型 */
    entityType?: string;
  };

  /**
   * 切换当前活动工程（**所有写工具只作用于当前活动工程**）；id 传空字符串返回主页。跨工程任务：只读查询优先用 get_project_state / get_team / get_timeline_summary 的 projectId，不必切换；需要写入时先切到目标工程，操作完再切回原工程，并重新读状态确认。
   */
  set_active_project: {
    /** 工程 id（用 list_projects 获取） */
    id: string;
  };

  /**
   * 临时启用 / 禁用 AI 上下文的某一个分段（只影响当前会话后续请求，不持久化，刷新即恢复）。segmentId 取自 get_ai_context_state 返回的 availableSegmentIds：persona（人设）/ passiveSkills（被动技能）/ activeSkills（主动技能清单）/ context（工程上下文）/ changes（变更队列）/ history（对话历史）/ user（用户输入）。enabled=false 即把该段从后续请求里剔除（用于压缩上下文、排障）；enabled=true 恢复注入。返回切换后的禁用清单。
   */
  set_ai_context_segment: {
    /** 上下文分段 id（） */
    segmentId: string;
    /** true=恢复注入，false=临时禁用（默认 false） */
    enabled?: boolean;
  };

  /**
   * 修改指定操作块的按键（按**序号**）。key 可用：普攻/重击/闪避/跳跃/共鸣技能/共鸣解放/声骸技能/谐度破坏 或字母。
   */
  set_block_key: {
    /** 操作块序号 */
    block: number;
    /** 新按键名 */
    key: string;
  };

  /**
   * 设置指定操作块的变奏标记（按**序号**）：intro=变奏入场、switchback=切回、none=取消。
   */
  set_block_special: {
    /** 操作块序号 */
    block: number;
    /** 特殊标记：none=无，intro=变奏（入场），switchback=切回 */
    kind: "none" | "intro" | "switchback";
  };

  /**
   * 设置工程 Buff 配置生效条件（全部满足才生效），传 null 清除。condition 结构：{"chain":共鸣链要求 0-6,"refinement":精炼要求 1-5,"elements":["伤害属性..."],"damageTypes":["伤害类型..."]}。全局工程 Buff 配置不可设置链/阶条件。
   */
  set_buff_conf_condition: {
    /** 工程 Buff 配置序号 */
    buffConf: number;
    /** 条件定义或 null 清除 */
    condition?: {
      chain?: number;
      refinement?: number;
      elements?: string[];
      damageTypes?: string[];
    };
  };

  /**
   * 设置工程 Buff 配置的作用范围：all（全队）或槽位数组（如 [1,3] 表示仅 1、3 号位）。全局工程 Buff 配置不可修改范围。
   */
  set_buff_conf_scope: {
    /** 工程 Buff 配置序号 */
    buffConf: number;
    /** all 或槽位数组 [1-3] */
    scope: string | unknown[];
  };

  /**
   * 设置工程 Buff 配置内指定乘区的数值（百分数乘区填数值，如 15 表示 15%）。zoneId 不存在时自动创建。zoneId 可选：atkFlat/atkPct/hpFlat/hpPct/defFlat/defPct/critRate/critDmg/recharge/tuneBreakBoost/offTuneBuildupRate/bonusDmg/deepenDmg/resPen/defPen/defDown/dmgRedPen/resDown/tuneStrainLayer/unisonBoonLayer/finalDmg/dmgTakenInc/specialFinal1/sp
   */
  set_buff_conf_zone: {
    /** 工程 Buff 配置序号（见 get_buff_confs 的「[NN]」） */
    buffConf: number;
    /** 乘区 id（接受旧 id，会自动重映射） */
    zoneId: string;
    /** 数值 */
    value: number;
    /** 可选，是否覆盖其它 Buff 的同乘区 */
    override?: boolean;
  };

  /**
   * 设置某条工程 Buff 配置内指定乘区的**乘区级生效条件**，传 null 清除。只接受 elements（伤害属性）与 damageTypes（伤害类型）两类，可只给其中一类；condition 对象形如 {"elements":["冷凝","热熔"],"damageTypes":["普攻伤害","重击伤害"]}。取值边界：属性取自 game-terms 的 ELEMENTS、伤害类型取自 DAMAGE_TYPES（见参数说明里的完整可选值）。判定口径为**类内「或」、类间「与」**——同类多选任一命中即满足，两类都给了则必须同时满足。链条件（chains）与阶条件（refinements
   */
  set_buff_conf_zone_condition: {
    /** 工程 Buff 配置序号（见 get_buff_confs 的「[NN]」） */
    buffConf: number;
    /** 乘区 id（接受旧 id，会自动重映射） */
    zoneId: string;
    /** 乘区级条件（只认 elements/damageTypes），或 null 清除 */
    condition?: {
      /** 伤害属性多选（任一匹配即满足），可选： */
      elements?: string[];
      /** 伤害类型多选（任一匹配即满足），可选： */
      damageTypes?: string[];
    };
  };

  /**
   * 设置工程 Buff 配置内指定乘区的引用（跟随某角色的属性按百分比折算），ref 为 null 时清除引用。ref 结构：{"targetZoneId":"引用目标","pct":百分比,"characterIdx":槽位 1-3,"threshold":阈值,"lower"/"upper"/"discrete"/"divisor"/"multiplier"可选}。targetZoneId 可选：baseAtk/totalAtk/baseHp/totalHp/baseDef/totalDef/recharge/tuneBreakBoost/offTuneBuildupRate/critRate
   */
  set_buff_conf_zone_ref: {
    /** 工程 Buff 配置序号 */
    buffConf: number;
    /** 乘区 id（接受旧 id，会自动重映射） */
    zoneId: string;
    /** 引用定义或 null 清除 */
    ref?: {
      targetZoneId?: string;
      pct?: number;
      /** 引用角色槽位 1-3 */
      characterIdx?: number;
      threshold?: number;
      lower?: number;
      upper?: number;
      discrete?: boolean;
      divisor?: number;
      multiplier?: number;
    };
  };

  /**
   * 设置指定角色槽位（1-3）的共鸣链数（0-6）。
   */
  set_chain: {
    /** 角色槽位 1-3 */
    slot: number;
    /** 链数 0-6 */
    value: number;
  };

  /**
   * 设置指定角色槽位（1-3）第 N 个声骸（1-5）的 cost（3/4/5；改 cost 会重置主词条）。
   */
  set_echo_cost: {
    /** 角色槽位 1-3 */
    char: number;
    /** 声骸位 1-5 */
    slot: number;
    /** cost：3/4/5 */
    cost: number;
  };

  /**
   * 设置指定伤害条目的伤害类型列表（覆盖，按**序号**）。取值：普攻伤害/重击伤害/共鸣技能伤害/共鸣解放伤害/声骸技能伤害/变奏技能伤害/延奏技能伤害/协同攻击伤害/效应伤害/其它类型伤害。
   */
  set_entry_damage_types: {
    /** 伤害条目序号（见 get_damage_entries 的「[NN]」） */
    entry: number;
    /** 伤害类型列表（可空 = 清空） */
    damageTypes: string[];
  };

  /**
   * 修改单个按键图标映射。id 为操作动作 id（如 attack/heavypress/dodge/q/e/r/f/t/space 等）；blockKey 为显示的图标 key（如 MouseLeft/MouseLeftHold/MouseRight/Q/E/R/F/T/SpaceBar，重击默认使用独立的 MouseLeftHold）；physical 为物理按键（单个小写字母 a-z 或空格 " "）。传 reset=true 可恢复全部按键图标为默认（此时忽略 id 等其它参数）。
   */
  set_keymap_entry: {
    /** 操作动作 id（见 get_keymap 返回） */
    id?: string;
    /** 图标 key（如 MouseLeft/MouseLeftHold/Q/SpaceBar） */
    blockKey?: string;
    /** 物理按键（单个小写字母 a-z 或空格 " "） */
    physical?: string;
    /** true = 恢复默认按键图标映射 */
    reset?: boolean;
  };

  /**
   * 开启 / 关闭「长任务模式」。开启后，如果你一次回复结束但任务还没做完，系统会自动追加一条「继续推进」的提示让回合继续，你不需要让用户反复说「继续」；只有当你确认**整个任务已经完成**（或确认无法继续）时，才调用本工具传 enabled=false 关闭它。适用于需要多轮工具往返、大批量处理、长文档分析等一次回复做不完的任务；普通问答不要开启。用户按停止按钮会立即结束该状态。
   */
  set_long_task: {
    /** true=开启长任务模式，false=关闭（任务完成时务必关闭） */
    enabled: boolean;
    /** 可选：一句话说明当前长任务的目标 / 进度（会显示在状态行，并用于自动继续时的提示） */
    note?: string;
  };

  /**
   * 设置指定角色槽位（1-3）第 N 个声骸（1-5）的主词条。label 须为该声骸 cost 支持的主词条（可选列表与 cost 相关，先 set_echo_cost 或 get_config_summary 查看），传空字符串清空。不传 value 时使用该词条满级默认值。
   */
  set_main_stat: {
    /** 角色槽位 1-3 */
    char: number;
    /** 声骸位 1-5 */
    slot: number;
    /** 主词条名称，空字符串清空 */
    label?: string;
    /** 可选，覆盖默认满级值 */
    value?: number;
  };

  /**
   * 设置已启用记点的秒数（按**序号**）。mode=auto：按参考线命名自动推导（清空自定义覆盖，回到自动解析值；命名无时间片段则秒数为 null「未填写」）；mode=custom：手动填秒数（须 ≥ 0，负数/空串视为清除→null「未填写」）。若该参考线尚未启用，会先自动启用再设秒数。
   */
  set_ref_line_timing_seconds: {
    /** 参考线序号 */
    line: number;
    /** auto=自动推导（从命名解析），custom=自定义秒数 */
    mode: "auto" | "custom";
    /** 秒数（mode=custom 时必填，≥0；留空/负数→清除为 null 未填写） */
    seconds?: number;
  };

  /**
   * 设置指定角色槽位（1-3）的武器精炼阶数（0-5，0=未精炼、不触发专武 1-5 阶 buff）。
   */
  set_refinement: {
    /** 角色槽位（1-3） */
    slot: number;
    /** 阶数 0-5 */
    value: number;
  };

  /**
   * 修改允许 AI 控制的设置。key 白名单：theme_mode(dark/light)、theme_accent_hue(default=青色/orange=橘红/orangeyellow=橙黄/magenta=品红/cyan=青色别名/indigo=靛蓝/green=墨绿/mono=黑白 或 0-360 整数)、theme_background_image(http(s)/data:image 地址或空串清除；白天/黑夜各一张，写法为地址或 {mode:"light"|"dark", url}，缺省写当前主题那张)、theme_bg_image_effect(对象 {blur?:0-32,
   */
  set_setting: {
    /** 设置项 key（见描述中的白名单） */
    key: string;
    /** 目标值 */
    value: string | number | boolean;
  };

  /**
   * 修改单个界面快捷键绑定。id 为快捷键定义 id（见 get_shortcuts 返回）；key 为新的快捷键组合（如 "ctrl+s"、"shift+enter"、"a"）。修饰键（Ctrl/Shift/Alt）用 + 连接，主键小写。若与同组其他快捷键冲突将报错。传 reset=true 可恢复全部快捷键为默认（此时忽略 id/key）。
   */
  set_shortcut: {
    /** 快捷键定义 id */
    id?: string;
    /** 新快捷键组合（如 ctrl+s、a、shift+enter） */
    key?: string;
    /** true = 恢复默认快捷键绑定 */
    reset?: boolean;
  };

  /**
   * 设置指定槽位（1-3）的角色，或传空字符串清空该槽位（同时清空武器、首位声骸与触发套装）。角色名用 get_team_catalog 查询，不能与其它槽位重复；已有武器类型不匹配时自动清空武器。
   */
  set_team_character: {
    /** 槽位 1-3 */
    slot: number;
    /** 角色名，空字符串清空 */
    character?: string;
  };

  /**
   * 设置指定槽位（1-3）的首位声骸，或传空字符串清空（同时清空触发套装）。声骸名用 get_team_catalog 查询；若触发套装已满 5 件且声骸不属于任何已选套装（赫卡忒除外），会清空触发套装。
   */
  set_team_first_echo: {
    /** 槽位 1-3 */
    slot: number;
    /** 声骸名，空字符串清空 */
    echo?: string;
  };

  /**
   * 设置指定槽位（1-3）的触发套装（整体覆盖）。sets 为 [{name, pieces}]，套装名与支持件数用 get_team_catalog 查询，总有效件数 ≤5。**只传 {name,5} 即可**：工具会自动附带同套的 {name,2}（5 件套包含 2 件套效果），不要自己再补；5 件套占满全部部位，不能与其它套装同时传。满 5 件且首位声骸不属于任何已选套装（赫卡忒除外）时清空首位声骸。
   */
  set_team_trigger_sets: {
    /** 槽位 1-3 */
    slot: number;
    /** 触发套装列表（整体覆盖），总有效件数 ≤5 */
    sets: {
      /** 套装名（用 get_team_catalog 查询） */
      name?: string;
      /** 生效件数（2 或 5） */
      pieces?: number;
    }[];
  };

  /**
   * 设置指定槽位（1-3）的武器，或传空字符串清空。武器名用 get_team_catalog 查询，武器类型须与该槽位角色匹配。
   */
  set_team_weapon: {
    /** 槽位 1-3 */
    slot: number;
    /** 武器名，空字符串清空 */
    weapon?: string;
  };

  /**
   * 更新「当前工作摘要」—— 显示在助手悬浮窗标题上、给用户看的一句话。**每轮回复开始前先调用它**（这是硬性要求，见 system 的「当前工作摘要」段）：用一句话概括正在做什么、进行到哪一步。它面向用户而非自己的备忘，不要写寒暄、过程或本次回答的结论。仍在做同一件事就沿用以求稳定，转向新任务才改写。
   */
  set_work_summary: {
    /** 一句话工作摘要（建议 10~30 字），例如「给椿配装，正在对比两把武器」。传空串表示清空摘要。 */
    summary: string;
  };

  /**
   * 切换当前视图（阶段）：team=队伍配置、timeline=排轴、calculation=拉表、config=配装、result=结果页。切换后用户会看到相应界面。
   */
  switch_view: {
    /** 目标视图 */
    view: string;
  };

  /**
   * 从工坊同步最新 Buff 集到主页的本地 Buff 集。注意：会整体覆盖“来自工坊”的实体，且工坊中已下线的实体将被移除（自定义实体不受影响）。
   */
  sync_buff_set_from_share: Record<string, never>;

  /**
   * 从库街区同步当前账号下鸣潮角色「正在装配的声骸」，写成本地自定义词条方案（方案名「库街区同步」，同名覆盖、可重复同步）。声骸不齐的角色按实际装配写入（缺的槽位留空），只有完全没声骸的角色才跳过；上游只给「漂泊者」这类不带形态的名字时需要用户指定属性，工具链里会跳过并回报，让用户在预览弹窗里选。需要先在「设置 → 连接配置 → 库街区账号」登录（AI 可用 open_panel 打开 settings）；上游若缺少角色装配声骸接口会明确报错。
   */
  sync_substat_plans_from_kuro: Record<string, never>;

  /**
   * 从工坊同步全部角色的标准14词条集（只覆盖工坊来源的方案，本地修改与自定义方案不受影响；工坊已下线的会移除）。
   */
  sync_substat_plans_from_share: Record<string, never>;

  /**
   * 切换“可用Buff/全部Buff”过滤：开启时隐藏条件不匹配（链/阶低于配置、属性/类型对不上条目）的 Buff。
   */
  toggle_condition_mismatch_hide: Record<string, never>;

  /**
   * 切换指定伤害条目的单个伤害类型（加上或移除，按**序号**）。已勾选的会被移除 —— 要「确保勾上」请先用 get_damage_entries 看当前类型。
   */
  toggle_damage_type: {
    /** 伤害条目序号（见 get_damage_entries 的「[NN]」） */
    entry: number;
    /** 伤害类型（如 共鸣技能伤害） */
    damageType: string;
  };

  /**
   * 将已归档工程恢复显示。
   */
  unarchive_project: {
    /** 要恢复的已归档工程 id */
    id: string;
  };

  /**
   * 把指定工程 Buff 配置从指定伤害条目解除绑定（两个参数都填**序号**）。
   */
  unbind_buff_conf_from_entry: {
    /** 伤害条目序号 */
    entry: number;
    /** 工程 Buff 配置序号 */
    buffConf: number;
  };

  /**
   * 从指定操作块 / 参考线解绑**一条**直伤命中（保留该目标上的其它伤害）。先用 get_damage_bindings 查 hitIndex（1 起）。
   */
  unbind_damage_hit: {
    /** 目标类型（默认 block） */
    targetType?: "block" | "refLine";
    /** 目标序号（操作块或参考线） */
    block: number;
    /** 该目标直伤命中列表里的序号（1 起） */
    hitIndex: number;
  };

  /**
   * 撤销上一次**拉表（表格）**变更（工程 Buff 配置增删改、条目↔Buff 绑定、伤害类型勾选等）。只回退表格，不动排轴与词条；排轴请用 undo_timeline。没有可撤销的操作时 ok=false 并给出原因（不会静默成功）。
   */
  undo_table: Record<string, never>;

  /**
   * 撤销上一次排轴操作。
   */
  undo_timeline: Record<string, never>;

  /**
   * 解锁当前活动工程的指定环节及后续所有环节。
   */
  unlock_phase: {
    /** 要解锁的环节（其后的环节会一并解锁） */
    phase: string;
  };

  /**
   * 整体覆写主页的本地 Buff 集中指定实体的 Buff 列表；仅在用户明确要求维护 Buff 集时使用，不用于给工程配 Buff（该实体来源变为 custom）。buffs 结构：[{"buffName":"名称","scope":"self|self_except|team|effect_only","exclusive":false,"condition":{...可选},"zones":[{"zoneId":"乘区id","value":数值,"override":false,"ref":{...可选}}]}]。
   */
  update_buff_set_entity_buffs: {
    /** 实体类型：character/weapon/echo/1set-5set */
    entityType: string;
    /** 实体名称（中文） */
    entityName: string;
    /** 完整 Buff 列表（整体覆写，格式见工具描述） */
    buffs: Record<string, never>[];
  };

  /**
   * 修改一条自定义倍率（按角色 + 当前倍率名定位）。可改 newName（改名，仍须同角色内唯一）、flatValue、pctValue、pctUnit、element、hits；未提供的字段保持原值。修改后至少仍保留一个 >0 的固定值 / 百分比。
   */
  update_custom_hit: {
    /** 角色名（须在当前队伍中） */
    character: string;
    /** 要修改的自定义倍率当前名称 */
    hitName: string;
    /** 可选：改成的新名称 */
    newName?: string;
    /** 新的固定值 */
    flatValue?: number;
    /** 新的百分比数值 */
    pctValue?: number;
    /** 新的百分比单位 */
    pctUnit?: "攻击%" | "生命%" | "防御%" | "偏谐系数";
    /** 新的伤害属性 */
    element?: "物理" | "冷凝" | "热熔" | "导电" | "气动" | "衍射" | "湮灭";
    /** 新的段数（整数 1-999） */
    hits?: number;
  };

  /**
   * 修改某个操作块/参考线上**已有的一条直伤命中**的倍率、属性或段数（用 get_damage_bindings 查 hitIndex）。倍率的基础类型必须保持不变（只改数值/段数），否则会改变底层条目 id 并让 Buff 绑定失效 —— 这种情况会明确拒绝；改名请用 rename_damage_entry。
   */
  update_damage_hit: {
    /** 目标类型（默认 block） */
    targetType?: "block" | "refLine";
    /** 目标序号（操作块或参考线） */
    block: number;
    /** 该目标直伤命中列表里的序号（1 起） */
    hitIndex: number;
    /** 新的倍率串（如 "120%" / "80%攻击+20%生命"）；基础类型须与原来一致 */
    ratio?: string;
    /** 新的伤害属性 */
    element?: "物理" | "冷凝" | "热熔" | "导电" | "气动" | "衍射" | "湮灭";
    /** 新的段数（整数 1-999） */
    hits?: number;
  };

  /**
   * 修改敌人配置项：level（等级）、defense（防御）、dmgReduction（减伤，数值如 10 表示 10%）、type（BOSS/精英怪/小怪）。
   */
  update_enemy: {
    /** 要修改的配置项 */
    key: "level" | "defense" | "dmgReduction" | "type";
    /** 目标值：level/defense 为数值，dmgReduction 为百分比数值（10 表示 10%），type 为 BOSS/精英怪/小怪 */
    value: number | string;
  };

  /**
   * 修改敌人某元素抗性（百分比数值，如 10 表示 10%）。元素：物理/冷凝/热熔/导电/气动/衍射/湮灭。
   */
  update_resistance: {
    /** 元素名：物理/冷凝/热熔/导电/气动/衍射/湮灭 */
    element: string;
    /** 抗性百分比数值（如 10 表示 10%） */
    value: number;
  };

  /**
   * 修改一张已有技能卡（按当前 name 定位）。可改 newName（改名，内置技能不可改名）、description、body、mode、enabled；未提供的字段保持原值。这是管理内置技能（如「Buff 命名规则」「黑话词典」）的唯一入口：改其正文即改对应流程的口径，清空正文会让内置默认重新生效。
   */
  update_skill: {
    /** 要修改的技能当前名称 */
    name: string;
    /** 可选：改成的新名称（自定义技能才可改名） */
    newName?: string;
    /** 新的一句话描述 */
    description?: string;
    /** 新的技能正文（传空串表示清空；内置技能清空后回落内置默认） */
    body?: string;
    /** 新的技能类型：active=按需激活 / passive=常驻注入 */
    mode?: "active" | "passive";
    /** 是否启用 */
    enabled?: boolean;
  };

  /**
   * 修改指定角色槽位（1-3）第 N 个声骸（1-5）第 idx 条副词条（从 0 开始）的数值（百分数词条填数值，如 8 表示 8%）。
   */
  update_substat_value: {
    /** 角色槽位（1-3） */
    char: number;
    /** 声骸槽位（1-5） */
    slot: number;
    /** 副词条下标（从 0 开始） */
    idx: number;
    /** 新数值（百分数词条填数值，如 8 表示 8%） */
    value: number;
  };

  /**
   * 激活一张**主动**技能卡：返回该技能的完整正文，之后请严格按正文中的规范继续处理当前任务。名称必须来自技能清单（被动技能无需激活，其正文已常驻生效）。
   */
  use_skill: {
    /** 技能名（来自技能清单，需完全一致） */
    name: string;
  };
}

/** 所有可用的工具箱 WS 工具名。 */
export type CoconutToolName = keyof CoconutToolParams;
