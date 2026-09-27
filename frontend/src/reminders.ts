import { invoke } from "@tauri-apps/api/core";

type Modal = { body: HTMLElement; dismiss: (value?: string) => void };
export type ReminderFields = { date: string; time: string; zone: string; repeat: string; every: string; weekdays: string[]; last: boolean; until: string };
export function reminderSpec(fields: ReminderFields, id: string): string {
  const parts = [`${fields.date} ${fields.time}`, `tz=${fields.zone.trim()}`];
  if (fields.repeat !== "once") {
    parts.push(`repeat=${fields.repeat}`, `every=${fields.every}`);
    if (fields.repeat === "weekly" && fields.weekdays.length) parts.push(`weekdays=${fields.weekdays.join(",")}`);
    if (["monthly", "yearly"].includes(fields.repeat) && fields.last) parts.push("day=last");
    if (fields.until) parts.push(`until=${fields.until}`);
  }
  parts.push(`id=${id}`);
  return parts.join("; ");
}

export type ReminderDefinition = { id: string; title: string; offset: number; end: number; schedule: { start: string; tz: string; repeat: string; every: number; weekdays: number[]; last_day: boolean; until?: string } };
export function reminderComposer(modal: Modal, initialTitle = "", existing?: ReminderDefinition) {
  const form = document.createElement("form");
  form.className = "modal-form reminder-form";
  form.innerHTML = `<label>Reminder text<input name="title" autocomplete="off" placeholder="For example: Water the plants"></label>
    <div class="reminder-columns"><label>Date<input name="date" type="date" required></label><label>Time<input name="time" type="time" required></label></div>
    <label>Time zone<input name="zone" required autocomplete="off" placeholder="America/Los_Angeles"></label>
    <label>Repeat<select name="repeat"><option value="once">Never (one time)</option><option value="daily">Daily</option><option value="weekly">Weekly</option><option value="monthly">Monthly</option><option value="yearly">Yearly</option></select></label>
    <div class="reminder-recurrence" hidden><label>Every<input name="every" type="number" min="1" max="100" value="1" required></label>
    <fieldset class="reminder-weekdays" hidden><legend>On these weekdays</legend>${["Mon","Tue","Wed","Thu","Fri","Sat","Sun"].map(day => `<label><input type="checkbox" name="weekday" value="${day}">${day}</label>`).join("")}</fieldset>
    <label class="reminder-last" hidden><input type="checkbox" name="last">Last day of the month</label>
    <label>End date (optional)<input type="date" name="until"></label></div>
    <p class="reminder-help">Reminders become active after the note saves. Dates use the chosen time zone. Monthly dates that do not exist are skipped. Checking a task cancels its reminders.</p>
    <p class="reminder-result" role="status" aria-live="polite"></p>
    <div class="modal-actions"><button type="button" class="secondary">Cancel</button><button type="submit">Insert reminder</button></div>`;
  const input = (name: string) => form.elements.namedItem(name) as HTMLInputElement;
  const now = new Date(Date.now() + 3600000);
  const pad = (n: number) => String(n).padStart(2, "0");
  input("date").value = `${now.getFullYear()}-${pad(now.getMonth()+1)}-${pad(now.getDate())}`;
  input("time").value = `${pad(now.getHours())}:${pad(now.getMinutes())}`;
  input("zone").value = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  input("title").value = initialTitle;
  const days = ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"];
  let weekdayCustomized = Boolean(existing?.schedule.weekdays.length);
  function defaultWeekday() {
    if (weekdayCustomized) return;
    const value = input("date").value;
    const date = new Date(`${value}T12:00:00`);
    if (Number.isNaN(date.valueOf())) return;
    const weekday = ["Sun", ...days.slice(0,6)][date.getDay()];
    form.querySelectorAll<HTMLInputElement>("[name=weekday]").forEach(e => e.checked = e.value === weekday);
  }
  input("date").addEventListener("change", defaultWeekday);
  form.querySelectorAll<HTMLInputElement>("[name=weekday]").forEach(e => e.addEventListener("change", () => weekdayCustomized = true));
  function visibility() {
    const repeat = input("repeat").value;
    const recurring = repeat !== "once";
    form.querySelector<HTMLElement>(".reminder-recurrence")!.hidden = !recurring;
    input("every").disabled = input("until").disabled = !recurring;
    form.querySelector<HTMLElement>(".reminder-weekdays")!.hidden = repeat !== "weekly";
    form.querySelectorAll<HTMLInputElement>("[name=weekday]").forEach(e => e.disabled = repeat !== "weekly");
    form.querySelector<HTMLElement>(".reminder-last")!.hidden = !["monthly","yearly"].includes(repeat);
    input("last").disabled = !["monthly","yearly"].includes(repeat);
  }
  input("repeat").addEventListener("change", visibility);
  if (existing) {
    const spec = existing.schedule;
    input("title").value = existing.title;
    input("title").readOnly = true;
    input("title").title = "Edit reminder text directly in the note";
    input("date").value = spec.start.slice(0,10);
    input("time").value = spec.start.slice(11,16);
    input("zone").value = spec.tz; input("repeat").value = spec.repeat;
    input("every").value = String(spec.every); input("until").value = spec.until || "";
    input("last").checked = spec.last_day;
    form.querySelectorAll<HTMLInputElement>("[name=weekday]").forEach(e => e.checked = spec.weekdays.includes(days.indexOf(e.value)+1));
    form.querySelector<HTMLButtonElement>("[type=submit]")!.textContent = "Update reminder";
  }
  defaultWeekday(); visibility();
  form.querySelector(".secondary")!.addEventListener("click", () => modal.dismiss());
  let permissionAcknowledged = false;
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (!form.reportValidity()) return;
    const result = form.querySelector<HTMLElement>(".reminder-result")!;
    const submit = form.querySelector<HTMLButtonElement>("[type=submit]")!;
    submit.disabled = true;
    const spec = reminderSpec({ date: input("date").value, time: input("time").value, zone: input("zone").value,
      repeat: input("repeat").value, every: input("every").value, weekdays: Array.from(form.querySelectorAll<HTMLInputElement>("[name=weekday]:checked")).map(e => e.value),
      last: input("last").checked, until: input("until").value }, existing?.id ?? crypto.randomUUID());
    try {
      await invoke<string>("reminder_validate", { spec });
      if (!form.isConnected) return;
      // Request permission while the user is deliberately creating a reminder.
      // Denial does not discard their text: the reminder list exposes delivery health.
      const permission = await invoke<string>("reminder_permissions");
      if (!form.isConnected) return;
      if (!permissionAcknowledged && /denied|not granted|not allowed|disabled/i.test(permission)) {
        result.textContent = `${permission}. Your reminder can still be inserted. Enable notifications in system settings.`;
        submit.textContent = "Insert anyway";
        submit.disabled = false;
        permissionAcknowledged = true;
        return;
      }
      modal.dismiss(existing ? `@remind(${spec})` : `${input("title").value.trim()} @remind(${spec})`.trimStart());
    } catch (error) { result.textContent = String(error); submit.disabled = false; }
  });
  modal.body.append(form);
  input("title").focus();
}

export type ReminderItem = { workspace: string; file: string; id: string; key: string; title: string; schedule: string; next?: string; paused?: boolean };
export type ReminderStatus = { items: ReminderItem[]; workspaces?: { identity: string; paused: boolean; forgotten: boolean }[]; errors: string[]; platform: { message: string; limited_until?: string } };
type ListState = { query: string; filter: string; notificationsOpen: boolean; workspacesOpen: boolean };
const workspaceName = (identity: string) => identity.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || identity;

export async function reminderList(body: HTMLElement, open?: (item: ReminderItem, edit: boolean) => void, health?: (status: ReminderStatus) => void,
  state: ListState = { query: "", filter: "all", notificationsOpen: false, workspacesOpen: false }) {
  body.textContent = "Loading reminders…";
  body.classList.add("reminders-browser");
  const node = <K extends keyof HTMLElementTagNameMap>(tag: K, text?: string, className?: string) => {
    const element = document.createElement(tag);
    if (text !== undefined) element.textContent = text;
    if (className) element.className = className;
    return element;
  };
  try {
    const result = await invoke<ReminderStatus>("reminder_status");
    if (!body.isConnected) return;
    health?.(result);
    body.replaceChildren();
    const feedback = node("p", "", "reminder-error");
    feedback.setAttribute("role", "status"); feedback.hidden = true;
    const report = (error: unknown) => { feedback.hidden = false; feedback.textContent = String(error); };
    const refresh = () => reminderList(body, open, health, state);
    const toolbar = node("div", undefined, "reminder-toolbar");
    const count = node("span", `${result.items.length} reminder${result.items.length === 1 ? "" : "s"}`, "muted");
    const reload = node("button", "Refresh");
    reload.onclick = async () => {
      reload.disabled = true; reload.textContent = "Refreshing…";
      try { await invoke("reminder_rescan"); await refresh(); }
      catch (error) { report(error); reload.disabled = false; reload.textContent = "Refresh"; }
    };
    toolbar.append(count, reload); body.append(toolbar, feedback);
    if (result.platform.limited_until) {
      body.append(node("p", `Open Markerup before ${new Date(result.platform.limited_until).toLocaleString()} to replenish scheduled notifications.`, "reminder-warning"));
    }
    if (result.errors.length) {
      const errors = node("details", undefined, "reminder-warning");
      errors.append(node("summary", `${result.errors.length} issue${result.errors.length === 1 ? " needs" : "s need"} attention`));
      for (const error of result.errors) errors.append(node("p", error));
      body.append(errors);
    }
    // Permission/service failures stay visible even with notification details closed.
    if (/not allowed|not granted|denied|disabled|unavailable|failed/i.test(result.platform.message)) {
      body.append(node("p", result.platform.message, "reminder-warning"));
    }
    const search = node("input"); search.type = "search"; search.placeholder = "Find a reminder or note…";
    search.setAttribute("aria-label", "Search reminders"); search.value = state.query;
    const filters = node("div", undefined, "reminder-filters"); filters.setAttribute("role", "group"); filters.setAttribute("aria-label", "Filter reminders");
    const list = node("div", undefined, "reminder-results");
    const now = new Date();
    const groupOf = (item: ReminderItem) => item.paused ? "Paused" : item.next && new Date(item.next).getTime() > now.getTime() ? "Upcoming" : "Past / finished";
    const when = (value: string) => {
      const date = new Date(value);
      const tomorrow = new Date(now); tomorrow.setDate(tomorrow.getDate() + 1);
      const day = date.toDateString() === now.toDateString() ? "Today" : date.toDateString() === tomorrow.toDateString() ? "Tomorrow" : date.toLocaleDateString(undefined, { month: "short", day: "numeric", ...(date.getFullYear() !== now.getFullYear() ? { year: "numeric" as const } : {}) });
      return `${day} · ${date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`;
    };
    const render = () => {
      list.replaceChildren();
      const query = state.query.trim().toLocaleLowerCase();
      const items = result.items.filter(item =>
        (!query || `${item.title} ${item.file} ${item.workspace}`.toLocaleLowerCase().includes(query)) &&
        (state.filter === "all" || groupOf(item).toLowerCase() === state.filter));
      count.textContent = `${items.length}${items.length !== result.items.length ? ` of ${result.items.length}` : ""} reminder${items.length === 1 ? "" : "s"}`;
      filters.querySelectorAll("button").forEach(button => button.setAttribute("aria-pressed", String(button.dataset.filter === state.filter)));
      if (!items.length) {
        const empty = node("div", undefined, "reminder-empty");
        empty.append(node("strong", result.items.length ? "No matching reminders" : "No reminders yet"), node("p", result.items.length ? "Try another search or filter." : "Open a note and choose Insert → Reminder to create one."));
        list.append(empty); return;
      }
      for (const group of ["Upcoming", "Paused", "Past / finished"]) {
        const entries = items.filter(item => groupOf(item) === group).sort((a,b) => (a.next || "").localeCompare(b.next || "") || a.title.localeCompare(b.title));
        if (!entries.length) continue;
        list.append(node("h3", `${group} · ${entries.length}`, "reminder-group-title"));
        for (const item of entries) {
          const card = node("article", undefined, "reminder-card");
          const title = node("strong", item.title || "Untitled reminder");
          const next = node("p", group === "Upcoming" ? when(item.next!) : group === "Paused" ? "Paused" : "No upcoming occurrence", "reminder-when");
          const details = node("p", `${item.file} · ${workspaceName(item.workspace || "")}`, "reminder-note");
          details.title = `${item.workspace || ""}/${item.file}`;
          card.append(title, next, details, node("p", item.schedule, "reminder-rule"));
          if (open) {
            const actions = node("div", undefined, "reminder-row-actions");
            for (const [label, edit] of [["Open note", false], ["Edit reminder", true]] as const) {
              const button = node("button", label); button.onclick = () => open(item, edit); actions.append(button);
            }
            card.append(actions);
          }
          list.append(card);
        }
      }
    };
    for (const [value, label] of [["all", "All"], ["upcoming", "Upcoming"], ["paused", "Paused"]]) {
      const button = node("button", label); button.dataset.filter = value;
      button.onclick = () => { state.filter = value; render(); }; filters.append(button);
    }
    search.oninput = () => { state.query = search.value; render(); };
    body.append(search, filters, list); render();
    const notifications = node("details", undefined, "reminder-preferences");
    notifications.open = state.notificationsOpen;
    notifications.ontoggle = () => { if (notifications.isConnected) state.notificationsOpen = notifications.open; };
    notifications.append(node("summary", "Notifications"), node("p", result.platform.message, "muted"));
    const permission = node("button", "Notification permission");
    permission.onclick = async () => {
      permission.disabled = true;
      try { await invoke("reminder_permissions"); await refresh(); }
      catch (error) { report(error); permission.disabled = false; }
    };
    notifications.append(permission, node("p", "Save note edits to update reminders. Open Markerup on each device to pick up changes.", "muted"));
    body.append(notifications);
    const workspaces = node("details", undefined, "reminder-preferences");
    workspaces.open = state.workspacesOpen;
    workspaces.ontoggle = () => { if (workspaces.isConnected) state.workspacesOpen = workspaces.open; };
    workspaces.append(node("summary", "Workspaces"));
    for (const workspace of result.workspaces ?? []) {
      const section = node("section", undefined, "reminder-workspace");
      section.append(node("strong", workspaceName(workspace.identity)), node("p", workspace.identity, "reminder-note"), node("p", workspace.forgotten ? "Not tracked" : workspace.paused ? "Paused" : "Reminders enabled", "muted"));
      const actions = node("div", undefined, "reminder-row-actions");
      for (const [label, action] of [[workspace.paused ? "Resume reminders" : "Pause reminders", workspace.paused ? "resume" : "pause"], ...(!workspace.forgotten ? [["Forget workspace", "forget"]] : [])]) {
        const button = node("button", label); actions.append(button);
        button.onclick = async () => {
          if (action === "forget" && button.dataset.confirm !== "yes") {
            button.dataset.confirm = "yes"; button.textContent = "Confirm: stop tracking this workspace";
            const cancel = node("button", "Cancel"); actions.append(cancel);
            const explanation = node("p", "This stops tracking reminders in this workspace. Your notes will not be changed.", "muted"); section.append(explanation);
            cancel.onclick = () => { delete button.dataset.confirm; button.textContent = label; cancel.remove(); explanation.remove(); };
            return;
          }
          button.disabled = true;
          try { await invoke("reminder_workspace_action", { identity: workspace.identity, action }); await refresh(); }
          catch (error) { report(error); button.disabled = false; }
        };
      }
      section.append(actions); workspaces.append(section);
    }
    if (!result.workspaces?.length) workspaces.append(node("p", "Open a workspace to enable its reminders.", "muted"));
    body.append(workspaces);
  } catch (error) {
    body.replaceChildren(node("p", `Could not load reminders: ${error}`, "reminder-error"));
    const retry = node("button", "Try again"); retry.onclick = () => { void reminderList(body, open, health, state); }; body.append(retry);
  }
}
