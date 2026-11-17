// SPDX-License-Identifier: GPL-3.0-or-later
// Opt-in adaptation of client window controls to the external XR companion.
// No Qt headers, Qt DT_NEEDED, private object layout, threads or background IO.
#include <dlfcn.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstddef>

namespace {
using Visible = void (*)(void *, bool);
using Cast = void *(*)(const void *, void *);
using Getter = void *(*)(void *);
using EventFilter = bool (*)(void *, void *, void *);
thread_local unsigned adapting;
unsigned logged;

bool opted(const char *kind) {
    const char *value = std::getenv("XR_QT_EXTERNAL_CONTROLS");
    return value && std::strcmp(value, kind) == 0;
}

// A named already-loaded object also covers WPS' RTLD_LOCAL Qt modules. NOLOAD
// never introduces another Qt implementation. Close our transient handle again.
void *from_loaded(const char *object, const char *name) {
    void *handle = dlopen(object, RTLD_LAZY | RTLD_NOLOAD);
    if (!handle) return nullptr;
    void *symbol = dlsym(handle, name);
    dlclose(handle);
    return symbol;
}

void *lookup(const char *name, bool wps) {
    if (void *p = dlsym(RTLD_DEFAULT, name)) return p;
    const char *objects_system[] = {"libpeony.so.3", "libkysdk-qtwidgets.so.1", "libQt5Widgets.so.5", "libQt5Core.so.5"};
    const char *objects_wps[] = {"libkprometheus.so", "libQt5WidgetsKso.so.5", "libQt5CoreKso.so.5"};
    if (wps) {
        for (const char *object : objects_wps)
            if (void *p = from_loaded(object, name)) return p;
    } else {
        for (const char *object : objects_system)
            if (void *p = from_loaded(object, name)) return p;
    }
    return nullptr;
}

Visible original_visible(bool wps) {
    const char *name = wps ? "_ZN6kso_qt7QWidget10setVisibleEb" : "_ZN7QWidget10setVisibleEb";
    if (void *p = dlsym(RTLD_NEXT, name)) return reinterpret_cast<Visible>(p);
    return reinterpret_cast<Visible>(from_loaded(wps ? "libQt5WidgetsKso.so.5" : "libQt5Widgets.so.5", name));
}

bool instance(void *object, const char *meta_name, bool wps) {
    const char *cast_name = wps ? "_ZNK6kso_qt11QMetaObject4castEPNS_7QObjectE" : "_ZNK11QMetaObject4castEP7QObject";
    auto cast = reinterpret_cast<Cast>(lookup(cast_name, wps));
    void *meta = lookup(meta_name, wps);
    return object && cast && meta && cast(meta, object);
}

void report(const char *kind, const char *control) {
    if (logged++ < 24)
        std::fprintf(stderr, "XR_QT_EXTERNAL_CONTROLS app=%s hidden=%s preserved=application-tabs,menus,toolbars\n", kind, control);
}

void hide_peony_buttons(void *bar) {
    auto visible = original_visible(false);
    if (!visible) return;
    const struct {const char *symbol; const char *label;} buttons[] = {
        {"_ZN3kdk16KWindowButtonBar13minimumButtonEv", "kdk::KWindowButtonBar.minimumButton"},
        {"_ZN3kdk16KWindowButtonBar13maximumButtonEv", "kdk::KWindowButtonBar.maximumButton"},
        {"_ZN3kdk16KWindowButtonBar11closeButtonEv", "kdk::KWindowButtonBar.closeButton"},
    };
    for (const auto &entry : buttons) {
        auto getter = reinterpret_cast<Getter>(lookup(entry.symbol, false));
        if (getter) if (void *button = getter(bar)) {
            visible(button, false);
            report("peony", entry.label);
        }
    }
    // The separate menuButton getter is deliberately never invoked or hidden.
}

bool peony_bar(void *object) {
    if (!instance(object, "_ZN3kdk16KWindowButtonBar16staticMetaObjectE", false)) return false;
    auto window = reinterpret_cast<Getter>(lookup("_ZNK7QWidget6windowEv", false));
    return window && instance(window(object), "_ZN5Peony8FMWindow16staticMetaObjectE", false);
}

const char *wps_window_button(void *object) {
    // Tab-close is independent application functionality, never a window control.
    if (instance(object, "_ZN28KPromeTabDelegateCloseButton16staticMetaObjectE", true)) return nullptr;
    if (!instance(object, "_ZN23KPromeWindowTitleButton16staticMetaObjectE", true)) return nullptr;
    auto window = reinterpret_cast<Getter>(lookup("_ZNK6kso_qt7QWidget6windowEv", true));
    if (!window || !instance(window(object), "_ZN16KPromeMainWindow16staticMetaObjectE", true)) return nullptr;
    // These subclasses have RTTI but no separate Q_OBJECT metadata. Use the
    // public C++ ABI cast instead of guessing private fields or broad base types.
    using DynamicCast = void *(*)(const void *, const void *, const void *, std::ptrdiff_t);
    auto cast = reinterpret_cast<DynamicCast>(lookup("__dynamic_cast", true));
    void *source = lookup("_ZTIN6kso_qt7QWidgetE", true);
    if (!cast || !source) return nullptr;
    const struct {const char *symbol; const char *label;} buttons[] = {
        {"_ZTI15KPromeMinButton", "KPromeMinButton"},
        {"_ZTI22KPromeRestoreMaxButton", "KPromeRestoreMaxButton"},
        {"_ZTI17KPromeCloseButton", "KPromeCloseButton"},
    };
    for (const auto &entry : buttons) {
        void *type = lookup(entry.symbol, true);
        if (type && cast(object, source, type, -1)) return entry.label;
    }
    return nullptr;
}
} // namespace

extern "C" void xr_system_visible(void *, bool) __asm__("_ZN7QWidget10setVisibleEb");
extern "C" void xr_system_visible(void *object, bool visible) {
    auto original = original_visible(false);
    if (!original) return; // Unreachable while Qt is executing this function.
    if (!adapting && visible && opted("peony") && peony_bar(object)) {
        ++adapting;
        hide_peony_buttons(object);
        original(object, visible);
        hide_peony_buttons(object);
        --adapting;
    } else original(object, visible);
}

extern "C" bool xr_peony_events(void *, void *, void *) __asm__("_ZN3kdk16KWindowButtonBar11eventFilterEP7QObjectP6QEvent");
extern "C" bool xr_peony_events(void *object, void *watched, void *event) {
    auto original = reinterpret_cast<EventFilter>(dlsym(RTLD_NEXT, "_ZN3kdk16KWindowButtonBar11eventFilterEP7QObjectP6QEvent"));
    if (!original) original = reinterpret_cast<EventFilter>(from_loaded("libkysdk-qtwidgets.so.1", "_ZN3kdk16KWindowButtonBar11eventFilterEP7QObjectP6QEvent"));
    bool result = original ? original(object, watched, event) : false;
    if (!adapting && opted("peony") && peony_bar(object)) {
        ++adapting;
        hide_peony_buttons(object);
        --adapting;
    }
    return result;
}

extern "C" void xr_wps_visible(void *, bool) __asm__("_ZN6kso_qt7QWidget10setVisibleEb");
extern "C" void xr_wps_visible(void *object, bool visible) {
    auto original = original_visible(true);
    if (!original) return;
    if (!adapting && visible && opted("wps")) {
        if (const char *button = wps_window_button(object)) {
            ++adapting;
            original(object, false);
            report("wps", button);
            --adapting;
            return;
        }
    }
    original(object, visible);
}
