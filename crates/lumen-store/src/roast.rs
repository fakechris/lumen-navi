//! Shared roast prompt construction — used by the desktop's manual
//! 「Roast my day」 and the daemon's automatic previous-day generation, so
//! both speak the same data semantics (attribution rules, coverage caveats).

use lumen_api::DayRoastSummaryDto;

fn fmt_dur(ms: i64) -> String {
    let s = ms / 1000;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}小时{m}分钟")
    } else if m > 0 {
        format!("{m}分钟{sec}秒")
    } else {
        format!("{sec}秒")
    }
}

/// Curated, semantics-annotated payload for the roast prompt. Raw DTO JSON
/// leaks sampler artifacts (screenshot counts, seen counts) that LLMs happily
/// mis-attribute to the user — this view states what each number MEANS.
pub fn prompt_data(summary: &DayRoastSummaryDto) -> serde_json::Value {
    let pct = |part: i64, whole: i64| -> Option<f64> {
        if whole <= 0 {
            None
        } else {
            Some(((part as f64 / whole as f64) * 1000.0).round() / 10.0)
        }
    };
    let covered = summary.total_active_ms.saturating_add(summary.total_idle_ms);
    let partial_day = covered > 0 && covered < 4 * 60 * 60 * 1000;
    serde_json::json!({
        "日期": summary.day,
        "覆盖说明": if partial_day {
            "下列前台/挂机/键鼠数字只覆盖监控开启后的窗口，不是日历日 24 小时。禁止写成「你今天只上了 N 分钟电脑」。"
        } else {
            "下列数字按当天本地日汇总。"
        },
        "行为归因信号": summary.attribution,
        "用户键鼠活跃": summary.user_active_ms.map(fmt_dur),
        "前台停留合计": fmt_dur(summary.total_active_ms),
        "挂机合计": fmt_dur(summary.total_idle_ms),
        "窗口切换": {
            "非空闲切换总数": summary.context_switches,
            "用户操作引起": summary.switches_user,
            "被动或程序引起": summary.switches_passive,
        },
        "键鼠输入计数": summary.input_counts,
        "应用TOP": summary.top_apps.iter().map(|a| serde_json::json!({
            "应用": a.app,
            "前台停留": fmt_dur(a.ms),
            "占前台合计%": a.pct,
            "用户键鼠活跃": fmt_dur(a.user_active_ms),
            "用户活跃占比%": pct(a.user_active_ms, a.ms),
            "短段高活跃": a.ms < 5 * 60_000 && pct(a.user_active_ms, a.ms).unwrap_or(0.0) >= 90.0,
        })).collect::<Vec<_>>(),
        "窗口标题TOP": summary.notable_titles.iter().map(|t| serde_json::json!({
            "应用": t.app,
            "标题": t.title,
            "前台停留": fmt_dur(t.dwell_ms),
            "用户键鼠活跃": fmt_dur(t.user_active_ms),
            "用户活跃占比%": pct(t.user_active_ms, t.dwell_ms),
            "鼠标点击": t.clicks,
            "回车提交": t.submits,
            "快捷键": t.shortcuts,
        })).collect::<Vec<_>>(),
        "域名TOP": summary.top_domains.iter().map(|d| serde_json::json!({
            "域名": d.domain,
            "前台停留": fmt_dur(d.ms),
        })).collect::<Vec<_>>(),
        "场景TOP": summary.top_scenes.iter().map(|s| serde_json::json!({
            "场景": s.label,
            "前台停留": fmt_dur(s.ms),
            "段数": s.episode_count,
        })).collect::<Vec<_>>(),
        "最忙小时": summary.busiest_hour.as_ref().map(|h| serde_json::json!({
            "小时": h.hour,
            "前台停留": fmt_dur(h.active_ms),
            "主要应用": h.top_app,
        })),
        "小时直方图": summary.hour_histogram.iter().map(|h| serde_json::json!({
            "小时": h.hour,
            "前台停留": fmt_dur(h.active_ms),
            "主要应用": h.top_app,
        })).collect::<Vec<_>>(),
        "pulse_score": summary.pulse_score,
        "采集元数据": {
            "自动截屏数": summary.screenshot_count,
            "AX采样数": summary.ax_sample_count,
            "说明": "系统定时采集的密度指标，与用户行为无关",
        },
    })
}

/// Full roast prompt. `tone` selects the persona: "advisor" (温和真诚建议 —
/// used by the automatic previous-day generation) or anything else (毒舌).
pub fn build_prompt(summary: &DayRoastSummaryDto, tone: &str) -> String {
    let (persona, style) = match tone {
        "advisor" => (
            "一位温和但观察敏锐的专注力教练",
            "- 语气真诚、有同理心，绝不嘲讽、不贴标签\n\
             - 每条 = 客观指出一个行为模式（引用具体数字）+ 一条可执行的具体建议\n\
             - 先讲事实，再给建议；肯定做得好的地方",
        ),
        _ => (
            "一个毒舌但洞察深刻的数字生活评论员",
            "- 语气幽默毒舌但不是人身攻击，吐槽行为模式而不是人格\n\
             - 可以玩梗，可以夸张，但数字必须来自数据\n\
             - 最后一条给一个真诚的建议",
        ),
    };

    let attribution_note = match summary.attribution.as_deref() {
        Some("interactions") => {
            "今天有精确交互事件（点击/提交/快捷键），user 键鼠活跃数据可信，放心使用 clicks/submits/快捷键 等计数。"
        }
        Some("input.stats") => {
            "今天只有聚合键鼠计数（分钟级粒度），user 活跃时长是区间估算，引用时用约数。"
        }
        _ => {
            "⚠️ 今天没有键鼠监控数据：你看到的时长全部只是「前台窗口停留」，完全无法区分用户主动操作和挂机/程序自动切换。这种情况下禁止断言用户的操作频率（例如「切了 N 次窗口」「看了 N 次」），只能说「某窗口停留了多久」，且要注明可能包含挂机。"
        }
    };

    let data = serde_json::to_string_pretty(&prompt_data(summary)).unwrap_or_default();
    format!(
        "你是{persona}。基于下面的 JSON 数据，写一份 6-10 条的中文点评。\n\
         要求：\n{style}\n\
         - 每条指向一个具体数字（百分比/次数/时长/标题）\n\
         - 时长必须抄数据里已经换算好的中文（如 16分钟、57秒、2小时47分钟），禁止写「毫秒」或未换算的大整数\n\
         - 直接输出内容，不要前言后语\n\n\
         【数据语义 — 必须遵守的因果规则】\n\
         1. 用户键鼠活跃、鼠标点击/回车提交/快捷键：唯一可信的主动行为证据。\n\
         2. 前台停留只代表窗口在前面，不等于专注。停留长 + 活跃占比低 ≈ 挂机/离开/看视频。\n\
         3. 窗口标题的前台停留不是「查看了 N 次」。\n\
         4. 谈切换只用「用户操作引起」；被动/程序引起的不能算到用户头上。\n\
         5. 自动截屏数/AX采样数是系统密度，严禁当作用户行为。\n\
         6. 覆盖说明必须遵守：监控只开了一段时，禁止把合计前台说成「今天只上了这么久电脑」。\n\
         7. 某应用前台短于 5 分钟且「短段高活跃」为 true：写成「这段很短且几乎都在操作」，禁止「全程在场 / 没有任何一秒是空的」。\n\
         8. 应用名是 Lumen Navi / Navi 时，点击多半是看自己的数据或改设置，不要夸成深度工作区。\n\
         9. 归因信号说明：{attribution_note}\n\n\
         数据：\n{data}"
    )
}
