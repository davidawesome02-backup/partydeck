#include <vector>
#include <string>
#include <cstdint>

std::string b32enc(const std::vector<uint8_t>& in) {
    static const char* A = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    std::string out;
    uint32_t buf = 0;
    int bits = 0;

    for (uint8_t b : in) {
        buf = (buf << 8) | b;
        bits += 8;
        while (bits >= 5) {
            out.push_back(A[(buf >> (bits - 5)) & 31]);
            bits -= 5;
        }
    }
    if (bits)
        out.push_back(A[(buf << (5 - bits)) & 31]);
    return out;
}

std::vector<uint8_t> b32dec(const std::string& in) {
    static const int8_t T[256] = {
        /* default */ -1,
        /* generated below */
    };
    static bool init = false;
    static int8_t table[256];

    if (!init) {
        for (int i = 0; i < 256; ++i) table[i] = -1;
        for (int i = 0; i < 26; ++i)
            table['A' + i] = table['a' + i] = i;
        for (int i = 0; i < 6; ++i)
            table['2' + i] = 26 + i;

        table['o'] = table['O'] = 0;
        table['i'] = table['I'] = 1;
        table['l'] = table['L'] = 1;
        table['s'] = table['S'] = 5;

        init = true;
    }

    std::vector<uint8_t> out;
    uint32_t buf = 0;
    int bits = 0;

    for (unsigned char c : in) {
        int v = table[c];
        if (v < 0) continue;
        buf = (buf << 5) | v;
        bits += 5;
        if (bits >= 8) {
            out.push_back((buf >> (bits - 8)) & 0xFF);
            bits -= 8;
        }
    }
    return out;
}