#pragma once
// Fixed worker wire and retained handles; see AGENTS.md.

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <ole2.h>
#include <bcrypt.h>
#include <UIAutomation.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <initializer_list>
#include <limits>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#pragma comment(lib, "bcrypt.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "oleaut32.lib")
#pragma comment(lib, "uiautomationcore.lib")
#pragma comment(lib, "user32.lib")
#pragma comment(lib, "advapi32.lib")
#pragma comment(lib, "dwmapi.lib")

namespace native_observer {

struct Refusal { const char* code; };
inline void require(bool value, const char* code) { if (!value) throw Refusal{code}; }
inline void checked(HRESULT value, const char* code = "provider_unavailable") { require(SUCCEEDED(value), code); }

class Handle {
    HANDLE value_ = nullptr;
public:
    Handle() = default;
    explicit Handle(HANDLE value) : value_(value) {}
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
    Handle(Handle&& other) noexcept : value_(std::exchange(other.value_, nullptr)) {}
    ~Handle() { if (value_ && value_ != INVALID_HANDLE_VALUE) CloseHandle(value_); }
    HANDLE get() const { return value_; }
};

template<class T> class Com {
    T* value_ = nullptr;
public:
    Com() = default;
    explicit Com(T* value) : value_(value) {}
    static Com retain(T* value) { if (value) value->AddRef(); return Com(value); }
    Com(const Com& other) : value_(other.value_) { if (value_) value_->AddRef(); }
    Com(Com&& other) noexcept : value_(std::exchange(other.value_, nullptr)) {}
    Com& operator=(Com other) noexcept { std::swap(value_, other.value_); return *this; }
    ~Com() { if (value_) value_->Release(); }
    T* get() const { return value_; }
    T* operator->() const { return value_; }
    explicit operator bool() const { return value_ != nullptr; }
    T** put() { require(!value_, "internal_refused"); return &value_; }
};

struct Bstr {
    BSTR value = nullptr;
    ~Bstr() { SysFreeString(value); }
    std::wstring bounded(size_t limit) const {
        auto size = SysStringLen(value);
        require(size <= limit, "property_bound");
        std::wstring result(value ? value : L"", size);
        require(result.find(L'\0') == std::wstring::npos, "property_invalid");
        return result;
    }
};

inline std::wstring wide(std::string_view text) {
    require(text.size() <= 65536, "request_bound");
    if (text.empty()) return {};
    int count = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(), static_cast<int>(text.size()), nullptr, 0);
    require(count > 0, "request_utf8");
    std::wstring result(count, L'\0');
    require(MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(), static_cast<int>(text.size()), result.data(), count) == count, "request_utf8");
    return result;
}

inline std::string utf8(std::wstring_view text) {
    if (text.empty()) return {};
    int count = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text.data(), static_cast<int>(text.size()), nullptr, 0, nullptr, nullptr);
    require(count > 0 && count <= 65536, "property_invalid");
    std::string result(count, '\0');
    require(WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text.data(), static_cast<int>(text.size()), result.data(), count, nullptr, nullptr) == count, "property_invalid");
    return result;
}

inline bool lower_hex(std::string_view text, size_t count) {
    return text.size() == count && std::all_of(text.begin(), text.end(), [](char c) { return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'); });
}

inline uint64_t decimal(std::string_view text, bool zero = false) {
    require(!text.empty() && text.size() <= 20 && (text.size() == 1 || text[0] != '0'), "wire_invalid");
    uint64_t result = 0;
    for (char c : text) {
        require(c >= '0' && c <= '9' && result <= (UINT64_MAX - static_cast<unsigned>(c - '0')) / 10, "wire_invalid");
        result = result * 10 + static_cast<unsigned>(c - '0');
    }
    require(zero || result != 0, "wire_invalid");
    return result;
}

inline std::string hex_number(uint64_t value, size_t count) {
    std::string result(count, '0');
    for (size_t i = count; i != 0; --i) { result[i - 1] = "0123456789abcdef"[value & 15]; value >>= 4; }
    require(value == 0, "wire_invalid");
    return result;
}

inline std::string sha256(std::string_view data) {
    BCRYPT_ALG_HANDLE algorithm = nullptr;
    require(BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) >= 0, "hash_unavailable");
    struct Algorithm { BCRYPT_ALG_HANDLE value; ~Algorithm() { BCryptCloseAlgorithmProvider(value, 0); } } owned{algorithm};
    std::array<UCHAR, 32> hash{};
    require(data.size() <= std::numeric_limits<ULONG>::max() && BCryptHash(algorithm, nullptr, 0,
        reinterpret_cast<PUCHAR>(const_cast<char*>(data.data())), static_cast<ULONG>(data.size()), hash.data(), static_cast<ULONG>(hash.size())) >= 0, "hash_unavailable");
    std::string result;
    for (auto byte : hash) result += hex_number(byte, 2);
    return result;
}

struct Atom {
    enum Kind { String, Number, Boolean } kind;
    std::string text;
    static Atom string(std::string value) { return {String, std::move(value)}; }
    static Atom number(uint64_t value) { return {Number, std::to_string(value)}; }
    static Atom boolean(bool value) { return {Boolean, value ? "true" : "false"}; }
};
using Object = std::map<std::string, Atom>;

class Json {
    std::string_view text_;
    size_t position_ = 0;
    char take() { require(position_ < text_.size(), "wire_invalid"); return text_[position_++]; }
    void whitespace() { while (position_ < text_.size() && (text_[position_] == ' ' || text_[position_] == '\r' || text_[position_] == '\n' || text_[position_] == '\t')) ++position_; }
    uint32_t code_unit() {
        uint32_t result = 0;
        for (int i = 0; i < 4; ++i) {
            char c = take();
            unsigned digit = c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : c >= 'A' && c <= 'F' ? c - 'A' + 10 : 16;
            require(digit < 16, "wire_invalid"); result = result * 16 + digit;
        }
        return result;
    }
    static void append(std::string& out, uint32_t point) {
        require(point != 0 && point <= 0x10ffff && !(point >= 0xd800 && point <= 0xdfff), "wire_invalid");
        if (point < 0x80) out += static_cast<char>(point);
        else if (point < 0x800) { out += static_cast<char>(0xc0 | (point >> 6)); out += static_cast<char>(0x80 | (point & 63)); }
        else if (point < 0x10000) { out += static_cast<char>(0xe0 | (point >> 12)); out += static_cast<char>(0x80 | ((point >> 6) & 63)); out += static_cast<char>(0x80 | (point & 63)); }
        else { out += static_cast<char>(0xf0 | (point >> 18)); out += static_cast<char>(0x80 | ((point >> 12) & 63)); out += static_cast<char>(0x80 | ((point >> 6) & 63)); out += static_cast<char>(0x80 | (point & 63)); }
    }
    std::string string() {
        require(take() == '"', "wire_invalid");
        std::string result;
        while (true) {
            char c = take();
            if (c == '"') break;
            require(static_cast<unsigned char>(c) >= 32, "wire_invalid");
            if (c != '\\') { result += c; continue; }
            c = take();
            if (c == '"' || c == '\\' || c == '/') result += c;
            else if (c == 'b') result += '\b'; else if (c == 'f') result += '\f';
            else if (c == 'n') result += '\n'; else if (c == 'r') result += '\r'; else if (c == 't') result += '\t';
            else if (c == 'u') {
                uint32_t point = code_unit();
                if (point >= 0xd800 && point <= 0xdbff) {
                    require(take() == '\\' && take() == 'u', "wire_invalid");
                    uint32_t low = code_unit(); require(low >= 0xdc00 && low <= 0xdfff, "wire_invalid");
                    point = 0x10000 + ((point - 0xd800) << 10) + low - 0xdc00;
                }
                append(result, point);
            } else throw Refusal{"wire_invalid"};
            require(result.size() <= 65536, "request_bound");
        }
        wide(result);
        return result;
    }
public:
    explicit Json(std::string_view text) : text_(text) { require(text.size() <= 65536, "request_bound"); wide(text); }
    Object object() {
        whitespace(); require(take() == '{', "wire_invalid"); whitespace();
        Object result;
        if (position_ < text_.size() && text_[position_] == '}') { ++position_; whitespace(); require(position_ == text_.size(), "wire_invalid"); return result; }
        while (true) {
            require(result.size() < 32, "wire_invalid");
            auto key = string(); whitespace(); require(take() == ':', "wire_invalid"); whitespace();
            Atom atom{Atom::String, {}};
            require(position_ < text_.size(), "wire_invalid");
            if (text_[position_] == '"') atom = Atom::string(string());
            else if (text_.substr(position_, 4) == "true") { position_ += 4; atom = Atom::boolean(true); }
            else if (text_.substr(position_, 5) == "false") { position_ += 5; atom = Atom::boolean(false); }
            else {
                size_t start = position_;
                while (position_ < text_.size() && text_[position_] >= '0' && text_[position_] <= '9') ++position_;
                atom = {Atom::Number, std::string(text_.substr(start, position_ - start))}; decimal(atom.text, true);
            }
            require(result.emplace(std::move(key), std::move(atom)).second, "wire_invalid"); whitespace();
            char c = take(); if (c == '}') break; require(c == ',', "wire_invalid"); whitespace();
        }
        whitespace(); require(position_ == text_.size(), "wire_invalid"); return result;
    }
};

inline std::string quote(std::string_view value) {
    std::string result = "\"";
    for (unsigned char c : value) {
        if (c == '"' || c == '\\') { result += '\\'; result += c; }
        else if (c < 32) { result += "\\u00"; result += hex_number(c, 2); }
        else result += c;
    }
    return result + '"';
}
inline std::string canonical(const Object& object) {
    std::string result = "{";
    for (const auto& [key, value] : object) {
        if (result.size() != 1) result += ',';
        result += quote(key) + ':' + (value.kind == Atom::String ? quote(value.text) : value.text);
    }
    return result + '}';
}
inline const std::string& field(const Object& object, const char* key, Atom::Kind kind = Atom::String) {
    auto found = object.find(key); require(found != object.end() && found->second.kind == kind, "wire_invalid"); return found->second.text;
}
inline void keys(const Object& object, std::initializer_list<const char*> expected) {
    require(object.size() == expected.size(), "wire_invalid");
    for (auto key : expected) require(object.count(key) == 1, "wire_invalid");
}

struct Request {
    bool invoke = false;
    DWORD pid = 0;
    uint64_t birth = 0, deadline = 0;
    HANDLE process = nullptr, ledger = nullptr;
    std::string run, operation, reservation_hash, ledger_file_id, exe_hash;
    std::wstring executable, expected_name;
    static Request parse(std::string_view bytes) {
        auto object = Json(bytes).object();
        keys(object, {"schema", "mode", "pid", "birth_filetime", "process_handle", "ledger_handle", "ledger_file_id", "operation", "reservation_sha256", "expected_name", "exe_path", "exe_sha256", "run_id", "deadline_tick"});
        require(field(object, "schema") == "fe-native-worker-v1", "wire_invalid");
        Request result;
        auto mode = field(object, "mode"); require(mode == "observe" || mode == "invoke_visible_petal_v1", "wire_invalid"); result.invoke = mode != "observe";
        auto pid = decimal(field(object, "pid", Atom::Number)); require(pid <= MAXDWORD, "wire_invalid"); result.pid = static_cast<DWORD>(pid);
        result.birth = decimal(field(object, "birth_filetime")); result.deadline = decimal(field(object, "deadline_tick"));
        result.process = reinterpret_cast<HANDLE>(static_cast<uintptr_t>(decimal(field(object, "process_handle"))));
        result.ledger = reinterpret_cast<HANDLE>(static_cast<uintptr_t>(decimal(field(object, "ledger_handle"), true)));
        result.run = field(object, "run_id"); result.operation = field(object, "operation");
        result.reservation_hash = field(object, "reservation_sha256"); result.ledger_file_id = field(object, "ledger_file_id");
        result.exe_hash = field(object, "exe_sha256"); result.executable = wide(field(object, "exe_path")); result.expected_name = wide(field(object, "expected_name"));
        require(lower_hex(result.run, 32) && lower_hex(result.operation, 32) && lower_hex(result.reservation_hash, 64) && lower_hex(result.exe_hash, 64) && lower_hex(result.ledger_file_id, 24), "wire_invalid");
        require(result.executable.size() > 3 && result.executable.size() < 32768 && result.executable[1] == L':' && result.executable[2] == L'\\' && result.executable.find(L'\0') == std::wstring::npos, "wire_invalid");
        require(result.expected_name.size() <= 256 && std::none_of(result.expected_name.begin(), result.expected_name.end(), [](wchar_t c) { return c < 32; }), "wire_invalid");
        require(result.invoke ? (result.ledger && !result.expected_name.empty() && result.operation != std::string(32, '0')) : (!result.ledger && result.expected_name.empty()), "wire_invalid");
        require(!result.invoke || (result.expected_name.starts_with(L"Open petal ") && result.expected_name.size() > 11), "wire_invalid");
        return result;
    }
};

inline std::string read_pipe() {
    std::string result;
    std::array<char, 4096> buffer{};
    DWORD count = 0;
    while (true) {
        if (!ReadFile(GetStdHandle(STD_INPUT_HANDLE), buffer.data(), static_cast<DWORD>(buffer.size()), &count, nullptr)) {
            require(GetLastError() == ERROR_BROKEN_PIPE, "request_io");
            break;
        }
        if (!count) break;
        require(result.size() + count <= 65536, "request_bound"); result.append(buffer.data(), count);
    }
    return result;
}

inline uint64_t filetime(const FILETIME& value) { return (static_cast<uint64_t>(value.dwHighDateTime) << 32) | value.dwLowDateTime; }

} // namespace native_observer
