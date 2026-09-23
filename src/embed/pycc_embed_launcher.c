/*
 * pycc embedded-executable launcher (Part 1 of #1028).
 *
 * The `main` of an embedded executable: it starts the CPython interpreter
 * bundled in the sidecar directory beside the executable, then runs the
 * pycc-compiled program as that interpreter's `__main__` module. It holds
 * interpreter *lifecycle* code only. Every PyObject* that crosses between
 * pycc code and CPython still goes through `src/ext/pycc_ext_module.c`,
 * compiled unchanged into the same link under the module name `__main__`
 * (the #1045 bridge split, recorded in the embedded-executable decision
 * entry under `docs/decisions/`).
 *
 * Unlike the shim, this file uses the non-limited API: `PyConfig_*` is not
 * part of the stable ABI. The two are separate translation units, so the
 * shim's own `Py_LIMITED_API` definition does not reach this one.
 */
#include <Python.h>
#include <limits.h>
#include <libgen.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#if defined(__APPLE__)
#include <mach-o/dyld.h>
#else
#include <unistd.h>
#endif

/* Defines PYCC_EMBED_SIDECAR: the sidecar directory's file name, fixed at
 * build time from `-o`'s file name (never derived from the running
 * executable's name, so renaming the executable keeps it working). Also
 * defines PYCC_EMBED_CLOSURE when the sidecar holds a locked dependency
 * closure in `closure/` (#1242). */
#include "pycc_embed_config.inc"

extern PyObject *PyInit___main__(void);

/* Writes the directory holding the running executable, symlinks resolved,
 * into `dir`. Returns 0 on success. */
static int pycc_embed_exe_dir(char *dir, size_t size) {
    char exe[PATH_MAX];
    char real[PATH_MAX];
#if defined(__APPLE__)
    uint32_t n = sizeof exe;
    if (_NSGetExecutablePath(exe, &n) != 0) {
        return -1;
    }
#else
    ssize_t n = readlink("/proc/self/exe", exe, sizeof exe - 1);
    if (n < 0) {
        return -1;
    }
    exe[n] = '\0';
#endif
    if (realpath(exe, real) == NULL) {
        return -1;
    }
    if ((size_t)snprintf(dir, size, "%s", dirname(real)) >= size) {
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    char dir[PATH_MAX];
    char home[PATH_MAX];
    if (pycc_embed_exe_dir(dir, sizeof dir) != 0 ||
        (size_t)snprintf(home, sizeof home, "%s/%s", dir, PYCC_EMBED_SIDECAR) >= sizeof home) {
        fprintf(stderr, "error: pycc could not locate its own executable to find %s\n",
                PYCC_EMBED_SIDECAR);
        return 1;
    }
    PyConfig config;
    PyConfig_InitIsolatedConfig(&config);
    config.site_import = 0;
    config.write_bytecode = 0;
    /* Unbuffered Python-side stdio is what keeps a Python-side write (for
     * example `pprint.pprint`) and a pycc `print` in program order: pycc_rt
     * writes through Rust's line-buffered stdout, which flushes at every
     * newline, and every pycc `print` ends in one while `print(end=...)` is
     * still refused (C0001). The pycc change that admits `end=` must add an
     * explicit Rust-side stdout flush before each foreign call (risk R3 of
     * the #1028 plan). */
    config.buffered_stdio = 0;
    PyStatus status = PyConfig_SetBytesString(&config, &config.home, home);
    /* The sidecar always holds the standard library (with its lib-dynload)
     * under `lib/`, whatever the build host's `sys.platlibdir` was (a
     * distribution build may say `lib64`), so the search path must not
     * inherit the compiled-in value. */
    if (!PyStatus_Exception(status)) {
        status = PyConfig_SetBytesString(&config, &config.platlibdir, "lib");
    }
    if (!PyStatus_Exception(status)) {
        status = PyConfig_SetBytesArgv(&config, argc, argv);
    }
    if (!PyStatus_Exception(status)) {
        status = Py_InitializeFromConfig(&config);
    }
    PyConfig_Clear(&config);
    if (PyStatus_Exception(status)) {
        Py_ExitStatusException(status);
    }
#ifdef PYCC_EMBED_CLOSURE
    /* The locked closure is appended after the bundled standard library,
     * so a closure package never shadows a standard-library module. */
    {
        char closure[PATH_MAX];
        PyObject *path = PySys_GetObject("path"); /* borrowed */
        PyObject *entry = NULL;
        if ((size_t)snprintf(closure, sizeof closure, "%s/closure", home) < sizeof closure) {
            entry = PyUnicode_DecodeFSDefault(closure);
        } else {
            PyErr_SetString(PyExc_OSError, "the closure directory's path is too long");
        }
        if (entry == NULL || path == NULL || PyList_Append(path, entry) != 0) {
            if (!PyErr_Occurred()) {
                PyErr_SetString(PyExc_RuntimeError, "sys.path is missing");
            }
            PyErr_Print();
            Py_XDECREF(entry);
            Py_FinalizeEx();
            return 1;
        }
        Py_DECREF(entry);
    }
#endif
    int rc = 1;
    PyObject *def = PyInit___main__();
    PyObject *machinery = def ? PyImport_ImportModule("importlib.machinery") : NULL;
    PyObject *spec =
        machinery ? PyObject_CallMethod(machinery, "ModuleSpec", "sO", "__main__", Py_None) : NULL;
    PyObject *module = spec ? PyModule_FromDefAndSpec((PyModuleDef *)def, spec) : NULL;
    if (module != NULL &&
        PyDict_SetItemString(PyImport_GetModuleDict(), "__main__", module) == 0 &&
        PyModule_ExecDef(module, (PyModuleDef *)def) == 0) {
        rc = 0;
    }
    if (rc != 0) {
        /* A `SystemExit` exits the process here with its own code. */
        PyErr_Print();
    }
    Py_XDECREF(module);
    Py_XDECREF(spec);
    Py_XDECREF(machinery);
    if (Py_FinalizeEx() < 0) {
        rc = 120;
    }
    return rc;
}
