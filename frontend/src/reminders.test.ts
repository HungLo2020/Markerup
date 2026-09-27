// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { reminderComposer, reminderSpec, reminderList } from "./reminders";

beforeEach(() => { document.body.replaceChildren(); invoke.mockReset(); });
function composer() {
  const body = document.createElement("div"); document.body.append(body);
  const dismiss = vi.fn(); reminderComposer({ body, dismiss });
  const form = body.querySelector("form")!;
  const input = (name: string) => form.elements.namedItem(name) as HTMLInputElement;
  return { body, dismiss, form, input };
}

test("serializes monthly and weekly rules without unrelated options", () => {
  const fields = { date: "2026-10-01", time: "09:30", zone: "America/Los_Angeles", repeat: "weekly", every: "2", weekdays: ["Mon","Fri"], last: true, until: "2027-02-01" };
  expect(reminderSpec(fields, "abc")).toBe("2026-10-01 09:30; tz=America/Los_Angeles; repeat=weekly; every=2; weekdays=Mon,Fri; until=2027-02-01; id=abc");
  expect(reminderSpec({ ...fields, repeat: "monthly" }, "abc")).toContain("day=last");
  expect(reminderSpec({ ...fields, repeat: "once" }, "abc")).toBe("2026-10-01 09:30; tz=America/Los_Angeles; id=abc");
});

test("composer validates through Rust and requests native permission before insertion", async () => {
  invoke.mockImplementation(async command => command === "reminder_validate" ? "2027-10-01T09:00:00Z" : "Allowed");
  const { input, form, dismiss } = composer();
  input("date").value = "2027-10-01"; input("time").value = "09:00"; input("zone").value = "UTC"; input("title").value = "Call Mom";
  input("repeat").value = "monthly"; input("repeat").dispatchEvent(new Event("change"));
  expect(form.querySelector<HTMLElement>(".reminder-recurrence")!.hidden).toBe(false);
  expect(form.querySelector<HTMLElement>(".reminder-weekdays")!.hidden).toBe(true);
  form.dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(dismiss).toHaveBeenCalledOnce());
  expect(dismiss.mock.calls[0][0]).toMatch(/^Call Mom @remind\(2027-10-01 09:00; tz=UTC; repeat=monthly; every=1; id=[a-f0-9-]+\)$/);
  expect(invoke.mock.calls.map(c => c[0])).toEqual(["reminder_validate", "reminder_permissions"]);
});

test("invalid schedules remain editable and permission denial requires explicit insertion", async () => {
  const { form, dismiss, input } = composer();
  invoke.mockRejectedValueOnce("Invalid time zone");
  form.dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(form.textContent).toContain("Invalid time zone"));
  expect(dismiss).not.toHaveBeenCalled();
  invoke.mockImplementation(async command => command === "reminder_permissions" ? "Notifications disabled" : "2027-01-01T00:00:00Z");
  form.dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(form.textContent).toContain("Insert anyway"));
  expect(dismiss).not.toHaveBeenCalled();
  input("title").value = "Updated title";
  form.dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(dismiss).toHaveBeenCalledOnce());
  expect(dismiss.mock.calls[0][0]).toContain("Updated title @remind(");
});

test("closing while validation is pending never inserts a reminder", async () => {
  let finish!: (v: string) => void;
  invoke.mockImplementation(() => new Promise<string>(resolve => { finish = resolve; }));
  const { body, form, dismiss } = composer();
  form.dispatchEvent(new Event("submit", { cancelable: true }));
  body.remove(); finish("valid");
  invoke.mockResolvedValue("Allowed");
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(dismiss).not.toHaveBeenCalled();
});

test("reminder list exposes scheduling limits and errors as text", async () => {
  const body = document.createElement("div"); document.body.append(body);
  invoke.mockResolvedValue({ platform: { message: "iOS schedules active", limited_until: "2027-01-01T00:00:00Z" }, errors: ["Note.md: Invalid date"], items: [{ file: "Note.md", title: "<img src=x onerror=alert(1)>", schedule: "Daily", next: "2026-12-01T00:00:00Z" }] });
  await reminderList(body);
  expect(body.textContent).toContain("Open Markerup before");
  expect(body.textContent).toContain("Invalid date");
  expect(body.querySelector("img")).toBeNull();
  expect(body.querySelector(".reminder-card")).not.toBeNull();
});

test("hidden recurrence controls never block a one-time reminder", async () => {
  invoke.mockResolvedValue("Allowed");
  const { input, form, dismiss } = composer();
  input("repeat").value = "daily"; input("repeat").dispatchEvent(new Event("change"));
  input("every").value = "";
  expect(form.checkValidity()).toBe(false);
  input("repeat").value = "once"; input("repeat").dispatchEvent(new Event("change"));
  expect(input("every").disabled).toBe(true);
  expect(form.checkValidity()).toBe(true);
  form.requestSubmit();
  await vi.waitFor(() => expect(dismiss).toHaveBeenCalledOnce());
});

test("weekly default follows the selected date until weekdays are customized", () => {
  const { input, form } = composer();
  input("repeat").value = "weekly"; input("repeat").dispatchEvent(new Event("change"));
  input("date").value = "2026-10-05"; input("date").dispatchEvent(new Event("change"));
  const selected = () => Array.from(form.querySelectorAll<HTMLInputElement>("[name=weekday]:checked")).map(e => e.value);
  expect(selected()).toEqual(["Mon"]);
  input("date").value = "2026-10-07"; input("date").dispatchEvent(new Event("change"));
  expect(selected()).toEqual(["Wed"]);
  const friday = form.querySelector<HTMLInputElement>('[value="Fri"]')!;
  friday.checked = true; friday.dispatchEvent(new Event("change"));
  input("date").value = "2026-10-08"; input("date").dispatchEvent(new Event("change"));
  expect(selected()).toEqual(["Wed", "Fri"]);
});

test("editing preserves the stable ID and replaces only the schedule marker", async () => {
  invoke.mockResolvedValue("Allowed");
  const body = document.createElement("div"); document.body.append(body);
  const dismiss = vi.fn();
  reminderComposer({body, dismiss}, "", { id:"stable", title:"日本語 task", offset:20,end:80,
    schedule:{start:"2027-10-01T09:00:00",tz:"UTC",repeat:"weekly",every:2,weekdays:[1,5],last_day:false,until:"2028-01-01"} });
  const form = body.querySelector("form")!;
  expect((form.elements.namedItem("date") as HTMLInputElement).value).toBe("2027-10-01");
  expect((form.elements.namedItem("title") as HTMLInputElement).readOnly).toBe(true);
  (form.elements.namedItem("time") as HTMLInputElement).value = "10:00";
  form.requestSubmit();
  await vi.waitFor(() => expect(dismiss).toHaveBeenCalledOnce());
  expect(dismiss.mock.calls[0][0]).toBe("@remind(2027-10-01 10:00; tz=UTC; repeat=weekly; every=2; weekdays=Mon,Fri; until=2028-01-01; id=stable)");
});

test("list opens and edits notes, pauses workspaces, and confirms forgetting", async () => {
  const body = document.createElement("div"); document.body.append(body);
  const item = {key:"key",id:"id",workspace:"workspace",file:"note.md",title:"Call",schedule:"Daily"};
  invoke.mockImplementation(async command => command === "reminder_status" ? {
    platform:{message:"Running",limited_until:"2027-10-01T00:00:00Z"},errors:[],items:[item],
    workspaces:[{identity:"workspace",paused:false,forgotten:false}]
  } : undefined);
  const open = vi.fn(), health = vi.fn();
  await reminderList(body, open, health);
  const click = (label: string) => Array.from(body.querySelectorAll("button")).find(e => e.textContent === label)!.click();
  click("Open note"); expect(open).toHaveBeenLastCalledWith(item,false);
  click("Edit reminder"); expect(open).toHaveBeenLastCalledWith(item,true);
  expect(health).toHaveBeenCalledOnce();
  click("Pause reminders");
  await vi.waitFor(() => expect(health).toHaveBeenCalledTimes(2));
  expect(invoke).toHaveBeenCalledWith("reminder_workspace_action",{identity:"workspace",action:"pause"});
  click("Forget workspace");
  expect(invoke).not.toHaveBeenCalledWith("reminder_workspace_action",{identity:"workspace",action:"forget"});
  click("Confirm: stop tracking this workspace");
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reminder_workspace_action",{identity:"workspace",action:"forget"}));
});
