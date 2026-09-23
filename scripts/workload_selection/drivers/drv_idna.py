import json, os
import idna
def setup():
    cases = json.load(open(os.path.join(os.path.dirname(__file__), "..", "inputs", "idna.json"), encoding="utf-8"))
    enc, dec, err = idna.encode, idna.decode, idna.IDNAError
    def run():
        for fn, args, kw, raises in cases:
            f = enc if fn == "encode" else dec
            if raises:
                try:
                    f(*args, **kw)
                except err:
                    pass
                else:
                    raise AssertionError("expected IDNAError")
            else:
                f(*args, **kw)
    return run
