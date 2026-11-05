// Ordinary owned UIA observation and one retained Invoke; see AGENTS.md.
#include "worker_protocol.hpp"
#include <dwmapi.h>

using namespace native_observer;

namespace {

struct ServiceSignatureFacts {
    DWORD style, extended;
    RECT outer, client;
};

struct ServiceSignatureRefusal : Refusal {
    ServiceSignatureFacts facts;
    ServiceSignatureRefusal(const char* code, const ServiceSignatureFacts& value) : Refusal{code}, facts(value) {}
};

struct Receipt {
    const char* code = "worker_refused";
    bool spent = true, invoke_entered = false, method_returned = false;
    bool output_attempted = false;
    bool parsed_observe = false;
    std::optional<ServiceSignatureFacts> service_signature;
    HRESULT method = S_OK;
    std::string root_hash = std::string(64, '0'), target_hash = std::string(64, '0');
    uint64_t watch_first = 0, watch_last = 0;
};

uint64_t worker_deadline() {
    auto now = GetTickCount64();
    require(now <= UINT64_MAX - 5000, "deadline_refused");
    return now + 5000;
}

class Deadline {
    Handle stopped_{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    std::atomic<uint64_t> end_{worker_deadline()};
    std::thread watchdog_;
public:
    Deadline() {
        require(stopped_.get() != nullptr, "watchdog_unavailable");
        watchdog_ = std::thread([this] {
            while (WaitForSingleObject(stopped_.get(), 10) == WAIT_TIMEOUT) {
                if (GetTickCount64() >= end_.load()) TerminateProcess(GetCurrentProcess(), 124);
            }
        });
    }
    ~Deadline() { SetEvent(stopped_.get()); if (watchdog_.joinable()) watchdog_.join(); }
    void bind(uint64_t end) {
        auto current = end_.load();
        require(end > GetTickCount64() && end <= current, "deadline_refused");
        end_.store(end);
    }
    void check() const { require(GetTickCount64() < end_.load(), "operation_deadline"); }
};

std::wstring object_name(HANDLE object) {
    DWORD bytes = 0;
    GetUserObjectInformationW(object, UOI_NAME, nullptr, 0, &bytes);
    require(bytes > sizeof(wchar_t) && bytes <= 512 && bytes % sizeof(wchar_t) == 0, "desktop_refused");
    std::vector<wchar_t> buffer(bytes / sizeof(wchar_t));
    require(GetUserObjectInformationW(object, UOI_NAME, buffer.data(), bytes, &bytes) != FALSE, "desktop_refused");
    require(buffer.back() == L'\0', "desktop_refused");
    return std::wstring(buffer.data());
}

std::vector<BYTE> token_information(HANDLE token, TOKEN_INFORMATION_CLASS kind) {
    DWORD bytes = 0;
    GetTokenInformation(token, kind, nullptr, 0, &bytes);
    require(bytes != 0 && bytes <= 65536, "token_identity_refused");
    std::vector<BYTE> data(bytes);
    require(GetTokenInformation(token, kind, data.data(), bytes, &bytes) != FALSE, "token_identity_refused");
    return data;
}

void token_identity(HANDLE app) {
    HANDLE self_raw = nullptr, app_raw = nullptr;
    require(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &self_raw) != FALSE, "token_identity_refused");
    Handle self{self_raw};
    require(OpenProcessToken(app, TOKEN_QUERY, &app_raw) != FALSE, "token_identity_refused");
    Handle other{app_raw};
    auto self_user = token_information(self.get(), TokenUser), app_user = token_information(other.get(), TokenUser);
    auto self_integrity = token_information(self.get(), TokenIntegrityLevel), app_integrity = token_information(other.get(), TokenIntegrityLevel);
    auto self_sid = reinterpret_cast<TOKEN_USER*>(self_user.data())->User.Sid;
    auto app_sid = reinterpret_cast<TOKEN_USER*>(app_user.data())->User.Sid;
    require(IsValidSid(self_sid) && IsValidSid(app_sid) && EqualSid(self_sid, app_sid), "token_identity_refused");
    auto integrity = [](std::vector<BYTE>& data) {
        auto sid = reinterpret_cast<TOKEN_MANDATORY_LABEL*>(data.data())->Label.Sid;
        require(IsValidSid(sid) && *GetSidSubAuthorityCount(sid) > 0, "token_identity_refused");
        return *GetSidSubAuthority(sid, *GetSidSubAuthorityCount(sid) - 1);
    };
    auto level = integrity(self_integrity);
    require(level == integrity(app_integrity) && level < SECURITY_MANDATORY_HIGH_RID, "integrity_refused");
    for (HANDLE token : {self.get(), other.get()}) {
        auto ui_access = token_information(token, TokenUIAccess);
        auto elevation = token_information(token, TokenElevation);
        require(*reinterpret_cast<DWORD*>(ui_access.data()) == 0
            && reinterpret_cast<TOKEN_ELEVATION*>(elevation.data())->TokenIsElevated == 0, "integrity_refused");
    }
}

class Process {
    const Request& request_;
public:
    explicit Process(const Request& request) : request_(request) {
        check();
        std::vector<wchar_t> image(32768);
        DWORD length = static_cast<DWORD>(image.size());
        require(QueryFullProcessImageNameW(request.process, 0, image.data(), &length) != FALSE
            && length == request.executable.size() && _wcsnicmp(image.data(), request.executable.c_str(), length) == 0, "process_identity_refused");
        DWORD own_session = 0, app_session = 0;
        require(ProcessIdToSessionId(GetCurrentProcessId(), &own_session) && ProcessIdToSessionId(request.pid, &app_session)
            && own_session != 0 && own_session == app_session, "session_refused");
        token_identity(request.process);
        require(object_name(GetProcessWindowStation()) == L"WinSta0"
            && object_name(GetThreadDesktop(GetCurrentThreadId())) == L"Default", "desktop_refused");
        HDESK input = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS);
        require(input != nullptr, "desktop_refused");
        auto name = object_name(input); CloseDesktop(input);
        require(name == L"Default", "desktop_refused");
    }
    void check() const {
        require(GetProcessId(request_.process) == request_.pid
            && WaitForSingleObject(request_.process, 0) == WAIT_TIMEOUT, "process_identity_refused");
        FILETIME created{}, ended{}, kernel{}, user{};
        require(GetProcessTimes(request_.process, &created, &ended, &kernel, &user)
            && filetime(created) == request_.birth, "process_identity_refused");
    }
};

class Watcher {
    static inline std::atomic<Watcher*> active_{nullptr};
    Handle ready_{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    Handle barrier_{CreateEventW(nullptr, FALSE, FALSE, nullptr)};
    std::thread loop_;
    std::atomic<DWORD> thread_{0};
    std::atomic<uint64_t> heartbeat_{0}, sequence_{0};
    std::atomic<uint64_t> next_barrier_{1}, completed_barrier_{0};
    std::atomic<bool> failed_{false}, installed_{false};
    std::array<std::pair<DWORD, HWND>, 256> events_{};
    Receipt& receipt_;
    static void CALLBACK event(HWINEVENTHOOK, DWORD kind, HWND window, LONG object, LONG child, DWORD, DWORD) {
        auto self = active_.load();
        if (!self || object != OBJID_WINDOW || child != CHILDID_SELF) return;
        auto index = self->sequence_.fetch_add(1);
        if (index >= self->events_.size()) { self->failed_.store(true); return; }
        self->events_[index] = {kind, window};
    }
public:
    Watcher(DWORD pid, Receipt& receipt) : receipt_(receipt) {
        require(ready_.get() && barrier_.get(), "watcher_unavailable");
        Watcher* empty = nullptr;
        require(active_.compare_exchange_strong(empty, this), "watcher_unavailable");
        try { loop_ = std::thread([this, pid] {
            MSG message{}; PeekMessageW(&message, nullptr, WM_USER, WM_USER, PM_NOREMOVE);
            thread_.store(GetCurrentThreadId());
            HWINEVENTHOOK hook = SetWinEventHook(EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, nullptr,
                &Watcher::event, pid, 0, WINEVENT_OUTOFCONTEXT);
            UINT_PTR timer = SetTimer(nullptr, 1, 25, nullptr);
            installed_.store(hook && timer);
            heartbeat_.store(GetTickCount64()); SetEvent(ready_.get());
            if (hook && timer) {
                while (true) {
                    BOOL result = GetMessageW(&message, nullptr, 0, 0);
                    if (result <= 0) { if (result < 0) failed_.store(true); break; }
                    heartbeat_.store(GetTickCount64());
                    if (message.message == WM_APP + 1) {
                        completed_barrier_.store(static_cast<uint64_t>(message.wParam));
                        SetEvent(barrier_.get());
                    }
                }
            }
            if (timer) KillTimer(nullptr, timer);
            if (hook) UnhookWinEvent(hook);
            installed_.store(false);
        }); } catch (...) { active_.store(nullptr); throw; }
        if (WaitForSingleObject(ready_.get(), 200) != WAIT_OBJECT_0 || !installed_.load()) {
            stop(); throw Refusal{"watcher_unavailable"};
        }
        try { check(); } catch (...) { stop(); throw; }
    }
    void stop() {
        auto thread = thread_.load();
        if (thread) PostThreadMessageW(thread, WM_QUIT, 0, 0);
        if (loop_.joinable()) loop_.join();
        receipt_.watch_last = sequence_.load();
        active_.store(nullptr);
    }
    ~Watcher() { stop(); }
    uint64_t sequence() const { return sequence_.load(); }
    void current() const {
        auto heartbeat = heartbeat_.load();
        auto now = GetTickCount64();
        require(installed_.load() && !failed_.load() && now >= heartbeat
            && now - heartbeat <= 100 && sequence_.load() == 0, "watcher_changed");
    }
    void check() {
        current();
        ResetEvent(barrier_.get());
        auto generation = next_barrier_.fetch_add(1);
        require(generation != 0 && generation <= INT64_MAX
            && PostThreadMessageW(thread_.load(), WM_APP + 1, static_cast<WPARAM>(generation), 0), "watcher_gap");
        auto start = GetTickCount64();
        require(start <= UINT64_MAX - 200, "watcher_gap");
        auto end = start + 200;
        while (completed_barrier_.load() < generation) {
            auto now = GetTickCount64();
            require(now < end && WaitForSingleObject(barrier_.get(), static_cast<DWORD>(end - now)) == WAIT_OBJECT_0, "watcher_gap");
        }
        current();
    }
};

std::wstring window_class(HWND window, const char* code = "window_identity_refused") {
    std::array<wchar_t, 256> name{};
    int count = GetClassNameW(window, name.data(), static_cast<int>(name.size()));
    require(count > 0 && count < static_cast<int>(name.size() - 1), code);
    return std::wstring(name.data(), count);
}

void no_native_children(HWND window, const char* code) {
    struct Children { unsigned count = 0; bool refused = false; } children;
    EnumChildWindows(window, [](HWND, LPARAM value) -> BOOL {
        auto data = reinterpret_cast<Children*>(value);
        if (++data->count > 128) { data->refused = true; return FALSE; }
        data->refused = true;
        return TRUE;
    }, reinterpret_cast<LPARAM>(&children));
    require(!children.refused && IsWindow(window), code);
}

void window_current(HWND window, DWORD pid) {
    DWORD actual = 0;
    DWORD thread = GetWindowThreadProcessId(window, &actual);
    LONG_PTR style = GetWindowLongPtrW(window, GWL_STYLE), extended = GetWindowLongPtrW(window, GWL_EXSTYLE);
    DWORD cloaked = 1;
    require(IsWindow(window) && thread && actual == pid && GetAncestor(window, GA_ROOT) == window
        && !GetWindow(window, GW_OWNER) && !(style & WS_CHILD) && !(extended & WS_EX_TOOLWINDOW)
        && IsWindowVisible(window) && !IsIconic(window) && window_class(window) == L"fractalengine"
        && SUCCEEDED(DwmGetWindowAttribute(window, DWMWA_CLOAKED, &cloaked, sizeof(cloaked))) && cloaked == 0
        && object_name(GetThreadDesktop(thread)) == L"Default", "window_identity_refused");
    no_native_children(window, "native_child_refused");
}

const char* winit_service_signature_refusal(DWORD style, DWORD extended, const RECT& outer, const RECT& client) {
    auto zero = [](const RECT& rect) { return rect.left == 0 && rect.top == 0 && rect.right == 0 && rect.bottom == 0; };
    constexpr DWORD expected_style = static_cast<DWORD>(WS_VISIBLE | WS_POPUP | WS_CLIPSIBLINGS);
    constexpr DWORD expected_extended = WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW;
    if (style != expected_style) return "window_service_style_refused";
    if (extended != expected_extended) return "window_service_exstyle_refused";
    if (outer.left != 0 || outer.top != 0 || outer.right != 16 || outer.bottom != 16) return "window_service_outer_rect_refused";
    if (!zero(client)) return "window_service_client_rect_refused";
    return nullptr;
}

Atom signed_coordinate(LONG value) {
    static_assert(sizeof(LONG) == sizeof(int32_t));
    return {Atom::Number, std::to_string(static_cast<int32_t>(value))};
}

std::string service_signature_json(const ServiceSignatureFacts& facts) {
    return canonical({
        {"schema", Atom::string("fe-native-service-signature-v2")},
        {"style_u32", Atom::number(facts.style)}, {"exstyle_u32", Atom::number(facts.extended)},
        {"outer_left_i32", signed_coordinate(facts.outer.left)},
        {"outer_top_i32", signed_coordinate(facts.outer.top)},
        {"outer_right_i32", signed_coordinate(facts.outer.right)},
        {"outer_bottom_i32", signed_coordinate(facts.outer.bottom)},
        {"client_left_i32", signed_coordinate(facts.client.left)},
        {"client_top_i32", signed_coordinate(facts.client.top)},
        {"client_right_i32", signed_coordinate(facts.client.right)},
        {"client_bottom_i32", signed_coordinate(facts.client.bottom)},
    });
}

void service_current(HWND window, DWORD pid, DWORD thread) {
    DWORD actual = 0;
    require(IsWindow(window) && thread && GetWindowThreadProcessId(window, &actual) == thread && actual == pid
        && GetAncestor(window, GA_ROOT) == window && !GetParent(window) && !GetWindow(window, GW_OWNER)
        && window_class(window, "window_service_identity_refused") == L"Winit Thread Event Target"
        && IsWindowVisible(window), "window_service_identity_refused");
    RECT outer{}, client{};
    require(GetWindowRect(window, &outer) && GetClientRect(window, &client), "window_service_geometry_refused");
    const DWORD style = static_cast<DWORD>(GetWindowLongPtrW(window, GWL_STYLE));
    const DWORD extended = static_cast<DWORD>(GetWindowLongPtrW(window, GWL_EXSTYLE));
    auto refusal = winit_service_signature_refusal(style, extended, outer, client);
    if (refusal) throw ServiceSignatureRefusal{refusal, {style, extended, outer, client}};
    no_native_children(window, "window_service_children_refused");
}

struct WindowSet {
    HWND root = nullptr, service = nullptr;
    DWORD root_thread = 0, service_thread = 0;
    void current(DWORD pid, Deadline& deadline) const;
};

WindowSet inspect_windows(DWORD pid, Deadline& deadline) {
    enum class Kind { Root, Service, Hidden };
    struct Entry { HWND window = nullptr; DWORD thread = 0; Kind kind = Kind::Hidden; };
    struct Windows {
        DWORD pid;
        std::array<Entry, 8> entries{};
        unsigned count = 0;
        const char* refusal = nullptr;
        void refuse(const char* code) { if (!refusal) refusal = code; }
    } windows{pid};
    BOOL complete = EnumWindows([](HWND window, LPARAM value) -> BOOL {
        auto data = reinterpret_cast<Windows*>(value);
        DWORD owner = 0;
        DWORD thread = GetWindowThreadProcessId(window, &owner);
        if (!thread) { data->refuse("window_enumeration_refused"); return FALSE; }
        if (owner != data->pid) return TRUE;
        if (++data->count > 8) { data->refuse("window_owned_count_refused"); return FALSE; }
        std::array<wchar_t, 256> name{};
        int count = GetClassNameW(window, name.data(), static_cast<int>(name.size()));
        if (count <= 0 || count >= static_cast<int>(name.size() - 1)) { data->refuse("window_class_query_refused"); return FALSE; }
        auto& entry = data->entries[data->count - 1];
        entry.window = window; entry.thread = thread;
        auto kind = std::wstring_view(name.data(), count);
        if (kind == L"fractalengine") entry.kind = Kind::Root;
        else if (kind == L"Winit Thread Event Target") entry.kind = Kind::Service;
        else if (IsWindowVisible(window)) data->refuse("window_unexpected_visible_refused");
        return TRUE;
    }, reinterpret_cast<LPARAM>(&windows));
    if (windows.refusal) throw Refusal{windows.refusal};
    require(complete, "window_enumeration_refused");
    WindowSet result;
    for (unsigned index = 0; index < windows.count; ++index) {
        deadline.check();
        const auto& entry = windows.entries[index];
        if (entry.kind == Kind::Root) {
            require(!result.root, "window_roots_multiple");
            result.root = entry.window; result.root_thread = entry.thread;
        } else if (entry.kind == Kind::Service) {
            require(!result.service, "window_services_multiple");
            result.service = entry.window; result.service_thread = entry.thread;
        }
    }
    require(result.root != nullptr, "window_root_missing");
    window_current(result.root, pid);
    require(GetWindowThreadProcessId(result.root, nullptr) == result.root_thread, "window_set_changed");
    if (result.service) {
        require(result.service_thread == result.root_thread, "window_service_identity_refused");
        service_current(result.service, pid, result.service_thread);
    }
    window_current(result.root, pid);
    deadline.check();
    return result;
}

void WindowSet::current(DWORD pid, Deadline& deadline) const {
    auto observed = inspect_windows(pid, deadline);
    require(observed.root == root && observed.root_thread == root_thread
        && observed.service == service && observed.service_thread == service_thread, "window_set_changed");
    window_current(root, pid);
    require(GetWindowThreadProcessId(root, nullptr) == root_thread, "window_set_changed");
    deadline.check();
}

struct Witness {
    uint64_t pid = 0, birth = 0, window = 0, context = 0, binding = 0, surface = 0, pass = 0, tick = 0;
    std::string profile, record;
    bool navigation = false;
    static Witness parse(const std::wstring& wide_record, const Request& request) {
        auto text = utf8(wide_record);
        require(text.size() <= 512 && std::all_of(text.begin(), text.end(), [](unsigned char c) { return c >= 32 && c <= 126; }), "witness_invalid");
        std::vector<std::string> parts;
        size_t start = 0;
        while (true) { auto end = text.find(';', start); parts.push_back(text.substr(start, end - start)); if (end == std::string::npos) break; start = end + 1; }
        require(parts.size() == 12 && parts[0] == "FEW1" && parts[11] == "complete=1", "witness_invalid");
        auto value = [&](size_t index, const char* name) {
            std::string prefix = std::string(name) + '=';
            require(parts[index].starts_with(prefix), "witness_invalid"); return parts[index].substr(prefix.size());
        };
        Witness result;
        result.pid = decimal(value(1, "p"));
        auto birth = value(2, "birth"); require(lower_hex(birth, 16), "witness_invalid");
        for (char c : birth) result.birth = result.birth * 16 + (c <= '9' ? c - '0' : c - 'a' + 10);
        result.window = decimal(value(3, "w")); result.context = decimal(value(4, "c")); result.binding = decimal(value(5, "b"));
        result.surface = decimal(value(6, "s")); result.pass = decimal(value(7, "pass")); result.tick = decimal(value(8, "tick"));
        auto classification = value(9, "class"); result.profile = value(10, "profile");
        result.navigation = classification == "navigation" && result.profile == "navigation_petal_v1";
        require(result.navigation || (classification == "scene" && result.profile == "scene_empty_v1"), "profile_refused");
        require(result.pid == request.pid && result.birth == request.birth && result.pid <= MAXDWORD
            && result.window <= INT64_MAX && result.context <= INT64_MAX && result.binding <= INT64_MAX && result.surface <= INT64_MAX && result.pass <= INT64_MAX, "witness_invalid");
        result.record = text;
        return result;
    }
    bool same_source(const Witness& other) const {
        return pid == other.pid && birth == other.birth && window == other.window && context == other.context
            && binding == other.binding && surface == other.surface && profile == other.profile
            && navigation == other.navigation && other.pass >= pass;
    }
    std::string stable_source() const {
        return std::to_string(pid) + '/' + std::to_string(birth) + '/' + std::to_string(window) + '/'
            + std::to_string(context) + '/' + std::to_string(binding) + '/' + std::to_string(surface) + '/' + profile;
    }
};

uint64_t property(HWND window) {
    auto value = reinterpret_cast<uintptr_t>(GetPropW(window, L"FractalEngine.NativeContext.v1"));
    require(value != 0 && value <= INT64_MAX, "property_unavailable");
    return value;
}

void neutral_input(HWND window) {
    require(GetForegroundWindow() == window, "input_competing");
    for (int key = 1; key < 255; ++key) require((GetAsyncKeyState(key) & 0x8000) == 0, "input_competing");
}

std::wstring automation_id(IUIAutomationElement* element) {
    Bstr value; checked(element->get_CurrentAutomationId(&value.value)); return value.bounded(128);
}

bool boolean_property(IUIAutomationElement* element, PROPERTYID property_id) {
    VARIANT value; VariantInit(&value);
    HRESULT status = element->GetCurrentPropertyValue(property_id, &value);
    if (FAILED(status)) { VariantClear(&value); throw Refusal{"provider_unavailable"}; }
    bool result = false;
    if (value.vt == VT_BOOL) result = value.boolVal != VARIANT_FALSE;
    else { VariantClear(&value); throw Refusal{"property_invalid"}; }
    VariantClear(&value); return result;
}

std::vector<int> runtime_id(IUIAutomationElement* element) {
    SAFEARRAY* array = nullptr;
    checked(element->GetRuntimeId(&array));
    struct Array { SAFEARRAY* value; ~Array() { if (value) SafeArrayDestroy(value); } } owned{array};
    require(array && SafeArrayGetDim(array) == 1, "runtime_identity_refused");
    LONG lower = 0, upper = -1;
    checked(SafeArrayGetLBound(array, 1, &lower)); checked(SafeArrayGetUBound(array, 1, &upper));
    VARTYPE type = VT_EMPTY; checked(SafeArrayGetVartype(array, &type));
    int64_t count = static_cast<int64_t>(upper) - static_cast<int64_t>(lower) + 1;
    require(type == VT_I4 && count > 0 && count <= 64, "runtime_identity_refused");
    std::vector<int> result;
    for (int64_t offset = 0; offset < count; ++offset) {
        LONG index = static_cast<LONG>(static_cast<int64_t>(lower) + offset);
        int value = 0; checked(SafeArrayGetElement(array, &index, &value)); result.push_back(value);
    }
    return result;
}

std::string runtime_text(const std::vector<int>& values) {
    std::string text;
    for (int value : values) text += std::to_string(value) + ',';
    return text;
}

bool same_element(IUIAutomation* automation, IUIAutomationElement* left, IUIAutomationElement* right) {
    BOOL same = FALSE; checked(automation->CompareElements(left, right, &same)); return same != FALSE;
}

bool positive(const RECT& rectangle) { return rectangle.left < rectangle.right && rectangle.top < rectangle.bottom; }
bool inside(const RECT& inner, const RECT& outer) {
    return positive(inner) && positive(outer) && inner.left >= outer.left && inner.top >= outer.top && inner.right <= outer.right && inner.bottom <= outer.bottom;
}
std::string rectangle_text(const RECT& rectangle) {
    return std::to_string(rectangle.left) + ',' + std::to_string(rectangle.top) + ',' + std::to_string(rectangle.right) + ',' + std::to_string(rectangle.bottom);
}

RECT client_rectangle(HWND window) {
    RECT rectangle{}; POINT origin{};
    require(GetClientRect(window, &rectangle) && ClientToScreen(window, &origin), "window_identity_refused");
    require(OffsetRect(&rectangle, origin.x, origin.y) && positive(rectangle), "window_identity_refused");
    return rectangle;
}

struct Tag {
    uint64_t set = 0, target = 0;
    static Tag parse(std::wstring_view value) {
        auto text = utf8(value);
        constexpr std::string_view prefix = "fe.nav.petal.v1;s=";
        require(text.starts_with(prefix), "target_tag_refused");
        auto split = text.find(";t=", prefix.size());
        require(split != std::string::npos && text.find(';', split + 1) == std::string::npos, "target_tag_refused");
        Tag result{decimal(std::string_view(text).substr(prefix.size(), split - prefix.size())), decimal(std::string_view(text).substr(split + 3))};
        require(result.set <= INT64_MAX && result.target <= INT64_MAX, "target_tag_refused");
        return result;
    }
};

struct Target {
    Com<IUIAutomationElement> element;
    Com<IUIAutomationInvokePattern> pattern;
    std::vector<int> runtime;
    std::wstring name, tag;
    RECT rectangle{};
    std::string ancestry, signature;
    Tag epochs;
};

struct Tree {
    Com<IUIAutomationElement> marked;
    std::vector<int> marked_runtime;
    std::string marked_ancestry;
    std::string marked_path;
    std::optional<Target> target;
    unsigned visited = 0, candidates = 0, matching = 0, original_seen = 0;
};

Tree walk(IUIAutomation* automation, IUIAutomationTreeWalker* walker,
    IUIAutomationElement* outer, HWND window, const Request& request, Deadline& deadline,
    const Target* retained = nullptr, IUIAutomationElement* retained_marked = nullptr) {
    struct Pending { Com<IUIAutomationElement> element; unsigned depth; std::string ancestors; bool enabled, visible; };
    std::vector<Pending> stack;
    stack.push_back({Com<IUIAutomationElement>::retain(outer), 0, {}, true, true});
    std::set<std::vector<int>> visited;
    Tree result;
    RECT client = client_rectangle(window);
    while (!stack.empty()) {
        deadline.check();
        auto current = std::move(stack.back()); stack.pop_back();
        require(current.depth <= 32 && result.visited < 1024, "tree_incomplete");
        ++result.visited;
        auto runtime = runtime_id(current.element.get());
        require(visited.insert(runtime).second, "tree_incomplete");
        auto marker = automation_id(current.element.get());
        require(!marker.starts_with(L"fe.surface.") && (!marker.starts_with(L"fe.native.") || marker == L"fe.native.window.v1"), "surface_refused");
        require(!boolean_property(current.element.get(), UIA_IsPasswordPropertyId)
            && !boolean_property(current.element.get(), UIA_WindowIsModalPropertyId), "surface_refused");
        int process = 0; CONTROLTYPEID type = 0; UIA_HWND native = nullptr;
        checked(current.element->get_CurrentProcessId(&process)); checked(current.element->get_CurrentControlType(&type));
        checked(current.element->get_CurrentNativeWindowHandle(&native));
        require(process == static_cast<int>(request.pid) && (!native || reinterpret_cast<HWND>(native) == window), "provider_identity_refused");
        BOOL enabled = FALSE, offscreen = TRUE;
        checked(current.element->get_CurrentIsEnabled(&enabled)); checked(current.element->get_CurrentIsOffscreen(&offscreen));
        bool available = current.enabled && enabled, visible = current.visible && !offscreen;
        std::string path = current.ancestors + '/' + runtime_text(runtime) + ':' + std::to_string(type) + ':' + utf8(marker);
        require(path.size() <= 32768, "tree_incomplete");
        if (marker == L"fe.native.window.v1") {
            require(!result.marked && type == UIA_WindowControlTypeId && available && visible, "root_ambiguous");
            require(!retained_marked || same_element(automation, retained_marked, current.element.get()), "root_identity_changed");
            result.marked = current.element;
            result.marked_runtime = runtime;
            result.marked_ancestry = current.ancestors;
            result.marked_path = path;
        }
        if (marker.starts_with(L"fe.nav.petal.v1;")) {
            require(++result.candidates <= 128 && type == UIA_ButtonControlTypeId, "target_bound");
            Tag epochs = Tag::parse(marker);
            if (request.invoke) {
                Bstr name; checked(current.element->get_CurrentName(&name.value));
                auto actual_name = name.bounded(256);
                if (actual_name == request.expected_name) {
                    require(++result.matching == 1, "target_ambiguous");
                    RECT bounds{}; checked(current.element->get_CurrentBoundingRectangle(&bounds));
                    require(available && visible && inside(bounds, client)
                        && boolean_property(current.element.get(), UIA_IsInvokePatternAvailablePropertyId), "target_unavailable");
                    if (retained) {
                        require(same_element(automation, retained->element.get(), current.element.get())
                            && runtime == retained->runtime && marker == retained->tag && current.ancestors == retained->ancestry
                            && std::memcmp(&bounds, &retained->rectangle, sizeof(bounds)) == 0, "target_identity_changed");
                        ++result.original_seen;
                    } else {
                        Target target;
                        target.element = current.element; target.runtime = runtime; target.name = actual_name;
                        target.tag = marker; target.rectangle = bounds; target.ancestry = current.ancestors; target.epochs = epochs;
                        checked(current.element->GetCurrentPatternAs(UIA_InvokePatternId, __uuidof(IUIAutomationInvokePattern), reinterpret_cast<void**>(target.pattern.put())));
                        require(static_cast<bool>(target.pattern), "target_unavailable");
                        target.signature = sha256(path + '|' + rectangle_text(bounds) + '|' + utf8(actual_name));
                        result.target = std::move(target);
                    }
                }
            }
        } else require(!marker.starts_with(L"fe.nav."), "target_tag_refused");
        Com<IUIAutomationElement> child;
        checked(walker->GetFirstChildElement(current.element.get(), child.put()));
        std::vector<Com<IUIAutomationElement>> children;
        while (child) {
            require(children.size() + stack.size() + result.visited < 1024, "tree_incomplete");
            children.push_back(child);
            Com<IUIAutomationElement> next;
            checked(walker->GetNextSiblingElement(child.get(), next.put()));
            child = std::move(next);
        }
        for (auto item = children.rbegin(); item != children.rend(); ++item)
            stack.push_back({*item, current.depth + 1, path, available, visible});
    }
    if (result.target || (retained && result.original_seen)) {
        const Target& target = result.target ? *result.target : *retained;
        require(result.marked && target.ancestry.starts_with(result.marked_path + '/'), "target_ancestry_refused");
        RECT root_bounds{}; checked(result.marked->get_CurrentBoundingRectangle(&root_bounds));
        require(inside(target.rectangle, root_bounds), "target_unavailable");
    }
    return result;
}

Witness root_witness(IUIAutomationElement* original, HWND window, const Request& request) {
    require(automation_id(original) == L"fe.native.window.v1", "root_identity_changed");
    CONTROLTYPEID type = 0; int process = 0; UIA_HWND native = nullptr;
    BOOL enabled = FALSE, offscreen = TRUE;
    checked(original->get_CurrentControlType(&type)); checked(original->get_CurrentProcessId(&process));
    checked(original->get_CurrentNativeWindowHandle(&native)); checked(original->get_CurrentIsEnabled(&enabled));
    checked(original->get_CurrentIsOffscreen(&offscreen));
    require(type == UIA_WindowControlTypeId && process == static_cast<int>(request.pid)
        && (!native || reinterpret_cast<HWND>(native) == window) && enabled && !offscreen
        && !boolean_property(original, UIA_IsPasswordPropertyId)
        && !boolean_property(original, UIA_WindowIsModalPropertyId), "root_identity_changed");
    uint64_t before = property(window);
    VARIANT value; VariantInit(&value);
    HRESULT status = original->GetCurrentPropertyValue(UIA_FullDescriptionPropertyId, &value);
    if (FAILED(status)) { VariantClear(&value); throw Refusal{"provider_unavailable"}; }
    if (value.vt != VT_BSTR || !value.bstrVal || SysStringLen(value.bstrVal) == 0) {
        VariantClear(&value); throw Refusal{"property_unavailable"};
    }
    if (SysStringLen(value.bstrVal) > 512) { VariantClear(&value); throw Refusal{"witness_invalid"}; }
    std::wstring text(value.bstrVal, SysStringLen(value.bstrVal)); VariantClear(&value);
    Witness result = Witness::parse(text, request);
    auto now = GetTickCount64();
    require(before == property(window) && before == result.binding && now >= result.tick && now - result.tick <= 250, "witness_stale");
    return result;
}

void original_target(const Target& target, const Request& request) {
    require(runtime_id(target.element.get()) == target.runtime && automation_id(target.element.get()) == target.tag, "target_identity_changed");
    Bstr name; checked(target.element->get_CurrentName(&name.value));
    CONTROLTYPEID type = 0; BOOL enabled = FALSE, offscreen = TRUE; RECT rectangle{};
    checked(target.element->get_CurrentControlType(&type)); checked(target.element->get_CurrentIsEnabled(&enabled));
    checked(target.element->get_CurrentIsOffscreen(&offscreen)); checked(target.element->get_CurrentBoundingRectangle(&rectangle));
    require(type == UIA_ButtonControlTypeId && name.bounded(256) == request.expected_name && enabled && !offscreen
        && std::memcmp(&rectangle, &target.rectangle, sizeof(rectangle)) == 0
        && boolean_property(target.element.get(), UIA_IsInvokePatternAvailablePropertyId), "target_identity_changed");
}

class Ledger {
    const Request& request_;
    std::string prefix_;
    void identity() const {
        BY_HANDLE_FILE_INFORMATION information{};
        require(GetFileType(request_.ledger) == FILE_TYPE_DISK
            && GetFileInformationByHandle(request_.ledger, &information)
            && !(information.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY))
            && information.nNumberOfLinks == 1
            && hex_number(information.dwVolumeSerialNumber, 8)
                + hex_number((static_cast<uint64_t>(information.nFileIndexHigh) << 32) | information.nFileIndexLow, 16)
                    == request_.ledger_file_id, "ledger_identity_refused");
    }
    std::string read() const {
        identity(); LARGE_INTEGER size{}, origin{};
        require(GetFileSizeEx(request_.ledger, &size) && size.QuadPart > 0 && size.QuadPart <= 65536
            && SetFilePointerEx(request_.ledger, origin, nullptr, FILE_BEGIN), "ledger_invalid");
        std::string result(static_cast<size_t>(size.QuadPart), '\0');
        DWORD count = 0;
        require(ReadFile(request_.ledger, result.data(), static_cast<DWORD>(result.size()), &count, nullptr)
            && count == result.size(), "ledger_io");
        identity(); return result;
    }
    static Object line(std::string_view bytes) {
        auto value = Json(bytes).object();
        require(canonical(value) == bytes, "ledger_noncanonical"); return value;
    }
public:
    explicit Ledger(const Request& request) : request_(request) {
        prefix_ = read();
        require(prefix_.ends_with('\n') && sha256(prefix_) == request.reservation_hash, "ledger_reservation_refused");
        auto first = prefix_.find('\n');
        require(first != std::string::npos && prefix_.find('\n', first + 1) == prefix_.size() - 1, "ledger_reservation_refused");
        auto bootstrap = line(std::string_view(prefix_).substr(0, first));
        auto reserved = line(std::string_view(prefix_).substr(first + 1, prefix_.size() - first - 2));
        keys(bootstrap, {"schema", "kind", "run_id", "ledger_id", "pid", "birth_filetime", "exe_sha256", "helper_sha256"});
        require(field(bootstrap, "schema") == "fe-native-attempt-v1" && field(bootstrap, "kind") == "bootstrap"
            && field(bootstrap, "run_id") == request.run && lower_hex(field(bootstrap, "ledger_id"), 32)
            && decimal(field(bootstrap, "pid", Atom::Number)) == request.pid
            && decimal(field(bootstrap, "birth_filetime")) == request.birth
            && field(bootstrap, "exe_sha256") == request.exe_hash && lower_hex(field(bootstrap, "helper_sha256"), 64), "ledger_binding_refused");
        keys(reserved, {"schema", "kind", "operation", "mode", "run_id", "pid", "birth_filetime", "expected_name_sha256"});
        require(field(reserved, "schema") == "fe-native-attempt-v1" && field(reserved, "kind") == "reserved"
            && field(reserved, "mode") == "invoke_visible_petal_v1" && field(reserved, "operation") == request.operation
            && field(reserved, "run_id") == request.run && decimal(field(reserved, "pid", Atom::Number)) == request.pid
            && decimal(field(reserved, "birth_filetime")) == request.birth
            && field(reserved, "expected_name_sha256") == sha256(utf8(request.expected_name)), "ledger_reservation_refused");
    }
    void arm(HWND window, const Witness& witness, const Target& target) {
        require(read() == prefix_, "ledger_changed");
        Object intent{
            {"schema", Atom::string("fe-native-attempt-v1")}, {"kind", Atom::string("intent")},
            {"operation", Atom::string(request_.operation)}, {"method", Atom::string("Invoke")},
            {"submission_possible", Atom::boolean(true)}, {"pid", Atom::number(request_.pid)},
            {"birth_filetime", Atom::string(std::to_string(request_.birth))},
            {"window", Atom::string(std::to_string(reinterpret_cast<uintptr_t>(window)))},
            {"w", Atom::string(std::to_string(witness.window))}, {"c", Atom::string(std::to_string(witness.context))},
            {"b", Atom::string(std::to_string(witness.binding))}, {"s", Atom::string(std::to_string(witness.surface))},
            {"pass", Atom::string(std::to_string(witness.pass))}, {"target_set", Atom::string(std::to_string(target.epochs.set))},
            {"target_epoch", Atom::string(std::to_string(target.epochs.target))}, {"target_sha256", Atom::string(target.signature)},
        };
        auto record = canonical(intent) + '\n';
        require(prefix_.size() + record.size() <= 65536, "ledger_bound");
        LARGE_INTEGER end{}; end.QuadPart = static_cast<LONGLONG>(prefix_.size());
        DWORD written = 0;
        require(SetFilePointerEx(request_.ledger, end, nullptr, FILE_BEGIN)
            && WriteFile(request_.ledger, record.data(), static_cast<DWORD>(record.size()), &written, nullptr)
            && written == record.size() && FlushFileBuffers(request_.ledger), "ledger_intent_unconfirmed");
        require(read() == prefix_ + record, "ledger_intent_unconfirmed");
    }
};

void dpi_context() {
    if (!AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)) {
        require(SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2), "dpi_refused");
        require(AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2), "dpi_refused");
    }
}

void emit(Receipt& receipt);

void operation(const Request& request, Deadline& deadline, Receipt& receipt) {
    Process process(request);
    std::optional<Ledger> ledger;
    if (request.invoke) ledger.emplace(request);
    dpi_context(); deadline.check();
    Watcher watcher(request.pid, receipt);
    receipt.watch_first = watcher.sequence();
    const WindowSet windows = inspect_windows(request.pid, deadline);
    HWND window = windows.root;
    auto boundary = [&] {
        deadline.check(); watcher.check(); process.check();
        windows.current(request.pid, deadline);
        watcher.current(); process.check(); deadline.check();
    };
    boundary();
    checked(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "com_unavailable");
    struct Apartment { ~Apartment() { CoUninitialize(); } } apartment;
    Com<IUIAutomation> automation;
    checked(CoCreateInstance(__uuidof(CUIAutomation), nullptr, CLSCTX_INPROC_SERVER,
        __uuidof(IUIAutomation), reinterpret_cast<void**>(automation.put())), "com_unavailable");
    Com<IUIAutomationTreeWalker> walker;
    checked(automation->get_RawViewWalker(walker.put()));
    Com<IUIAutomationElement> outer;
    checked(automation->ElementFromHandle(window, outer.put()));
    auto outer_runtime = runtime_id(outer.get());
    Request read_only = request;
    read_only.invoke = false;
    Tree discovered;
    std::optional<Witness> admitted;
    while (!admitted) {
        boundary();
        require(runtime_id(outer.get()) == outer_runtime, "root_identity_changed");
        discovered = walk(automation.get(), walker.get(), outer.get(), window, read_only, deadline);
        if (discovered.marked) {
            try { admitted = root_witness(discovered.marked.get(), window, request); }
            catch (const Refusal& refusal) {
                if (std::string_view(refusal.code) != "property_unavailable") throw;
            }
        }
        if (!admitted) Sleep(10);
    }
    Witness original = *admitted;
    auto marked = discovered.marked;
    auto marked_runtime = discovered.marked_runtime;
    auto marked_ancestry = discovered.marked_ancestry;
    receipt.root_hash = sha256(original.record);
    auto fresh_root = [&] {
        require(runtime_id(outer.get()) == outer_runtime && runtime_id(marked.get()) == marked_runtime, "root_identity_changed");
        auto current = root_witness(marked.get(), window, request);
        require(original.same_source(current), "source_changed");
        original = current;
        return current;
    };
    boundary(); fresh_root();
    if (!request.invoke) {
        auto validated = walk(automation.get(), walker.get(), outer.get(), window, read_only, deadline, nullptr, marked.get());
        require(validated.marked && validated.marked_runtime == marked_runtime && validated.marked_ancestry == marked_ancestry, "root_identity_changed");
        boundary(); auto current = fresh_root();
        receipt.root_hash = sha256(current.record); receipt.watch_last = watcher.sequence(); receipt.code = "observed";
        boundary();
        emit(receipt);
        return;
    }
    require(original.navigation, "profile_refused"); neutral_input(window);
    boundary();
    auto selected = walk(automation.get(), walker.get(), outer.get(), window, request, deadline, nullptr, marked.get());
    require(selected.marked && selected.marked_runtime == marked_runtime && selected.marked_ancestry == marked_ancestry
        && selected.matching == 1 && selected.target.has_value(), "target_unavailable");
    boundary();
    Target target = std::move(*selected.target);
    target.signature = sha256(original.stable_source() + '|' + target.signature);
    receipt.target_hash = target.signature;
    auto preflight = [&] {
        boundary(); fresh_root();
        original_target(target, request);
        auto validated = walk(automation.get(), walker.get(), outer.get(), window, request, deadline, &target, marked.get());
        require(validated.marked && validated.marked_runtime == marked_runtime && validated.marked_ancestry == marked_ancestry
            && validated.matching == 1 && validated.original_seen == 1, "target_identity_changed");
        boundary(); neutral_input(window); original_target(target, request);
        auto current = fresh_root();
        receipt.root_hash = sha256(current.record);
        boundary();
        return current;
    };
    auto intent_source = preflight();
    ledger->arm(window, intent_source, target);
    preflight();
    receipt.invoke_entered = true;
    receipt.method = target.pattern->Invoke();
    receipt.method_returned = true;
    receipt.code = SUCCEEDED(receipt.method) ? "submitted_outcome_unknown" : "invoke_failed_outcome_unknown";
    try {
        boundary(); original_target(target, request); fresh_root(); boundary();
    } catch (const Refusal&) {
        if (SUCCEEDED(receipt.method)) receipt.code = "submitted_state_changed";
    }
    receipt.watch_last = watcher.sequence();
    emit(receipt);
}

void emit(Receipt& receipt) {
    require(!receipt.output_attempted, "output_io");
    receipt.output_attempted = true;
    Object result{
        {"schema", Atom::string("fe-native-worker-result-v1")}, {"code", Atom::string(receipt.code)},
        {"attempt_spent", Atom::boolean(receipt.spent)}, {"invoke_entered", Atom::boolean(receipt.invoke_entered)},
        {"method_returned", Atom::boolean(receipt.method_returned)},
        {"hr", Atom::string(hex_number(static_cast<uint32_t>(receipt.method), 8))},
        {"root_sha256", Atom::string(receipt.root_hash)}, {"target_sha256", Atom::string(receipt.target_hash)},
        {"watch_first", Atom::number(receipt.watch_first)}, {"watch_last", Atom::number(receipt.watch_last)},
    };
    auto bytes = canonical(result);
    if (receipt.service_signature && receipt.parsed_observe && !receipt.spent && !receipt.invoke_entered
        && !receipt.method_returned && receipt.method == S_OK) {
        const auto& facts = *receipt.service_signature;
        const auto refusal = winit_service_signature_refusal(facts.style, facts.extended, facts.outer, facts.client);
        if (refusal && std::string_view(receipt.code) == refusal) {
            bytes.pop_back();
            bytes += ",\"service_signature\":" + service_signature_json(facts) + '}';
        }
    }
    bytes += '\n';
    require(bytes.size() <= 262144, "output_bound");
    DWORD written = 0;
    require(WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr)
        && written == bytes.size(), "output_io");
}

} // namespace

int wmain(int argc, wchar_t**) {
    static_assert(sizeof(void*) == 8, "native observer requires x64");
    Receipt receipt;
    std::unique_ptr<Deadline> deadline;
    try {
        deadline = std::make_unique<Deadline>();
        require(argc == 1, "wire_invalid");
        auto request = Request::parse(read_pipe());
        receipt.spent = request.invoke;
        receipt.parsed_observe = !request.invoke;
        deadline->bind(request.deadline);
        operation(request, *deadline, receipt);
    } catch (const ServiceSignatureRefusal& refusal) {
        receipt.code = refusal.code;
        if (receipt.parsed_observe) receipt.service_signature = refusal.facts;
    } catch (const Refusal& refusal) { receipt.code = refusal.code; }
    catch (...) { receipt.code = "worker_refused"; }
    if (!receipt.output_attempted) {
        try { emit(receipt); } catch (...) { return 3; }
    }
    return std::string_view(receipt.code) == "observed" || receipt.invoke_entered ? 0 : 2;
}
