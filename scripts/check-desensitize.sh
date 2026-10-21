#!/bin/sh
# 仓库脱敏守门：扫描**实机现场值**，防止真实地址 / 凭据重新进入仓库。
#
# 用法：sh scripts/check-desensitize.sh [目录，默认仓库根]
# 退出码：0 = 干净；1 = 发现可疑项（打印文件:行号 + 原因）
#
# 口径见 docs/PRIVACY.md。要点：
#   · 只允许**文档保留段**与**私有段**出现具体地址：
#       文档段：192.0.2.0/24、198.51.100.0/24、203.0.113.0/24、2001:db8::/32
#       私有段：10/8、172.16/12、192.168/16、127/8、169.254/16
#   · 历史实机网段（虽私有，但暴露用户网络拓扑）逐个列在下面；
#   · 运营商公网 IPv6 前缀（240e / 2409 / 2408 / 2400）一律不许出现；
#   · 凭据：ghp_ / github_pat_ / sk- / ssh 口令 / 真实穿透域名。
set -u
ROOT=${1:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)}
cd "$ROOT" || exit 2

FILES=$(find . -type f \
  \( -name '*.rs' -o -name '*.md' -o -name '*.toml' -o -name '*.sh' \
     -o -name '*.html' -o -name '*.js' -o -name '*.json' -o -name '*.uc' \) \
  -not -path './.git/*' -not -path './rust/target/*' \
  -not -path './scripts/*' 2>/dev/null)

fail=0
report() { fail=1; printf '\033[31m[脱敏]\033[0m %s\n' "$1"; printf '%s\n' "$2" | sed 's/^/    /'; }

# 1) 历史实机网段（私有地址，但能反推用户拓扑）
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '192\.168\.88\.|10\.4\.83\.|10\.161\.|10\.2\.98\.|10\.5\.23\.|10\.149\.|10\.7\.45\.' 2>/dev/null)
[ -n "$hits" ] && report "出现历史实机网段（示例请用 10.42.x.x / 192.168.1.x）" "$hits"

# 2) 运营商公网 IPv6 前缀
#
#    例外（2026-10-21）：`2400:3200::1` 是**阿里公共 IPv6 DNS**，与 v4 侧的 223.5.5.5
#    同族，不是运营商分配给用户的实机前缀 —— 它是接口默认下发 DNS 的一部分（见
#    `config::net::DEFAULT_WAN_DNS`），必须进仓库，故放行进白名单。
#    `2402:4e00::`（DNSPod 公共 DNS）前缀不在上面四个运营商段里，本就不会命中。
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '\b(240e|2409|2408|2400):' 2>/dev/null \
  | grep -v '2400:3200::1')
[ -n "$hits" ] && report "出现运营商公网 IPv6（示例请用 2001:db8::/32）" "$hits"

# 3) 合法但非文档段的公网 IPv4（逐段判 <=255，避免把 999.1.1.1、1.2.3.256 这类脏值当地址）
#
#    白名单里除文档占位值外还有三个**公共递归 DNS**（223.5.5.5 阿里、119.29.29.29
#    DNSPod、114.114.114.114）：它们随默认配置下发，不是实机现场值。
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' 2>/dev/null \
  | grep -vE '([0-9]{1,3}\.){7,}[0-9]{1,3}' \
  | grep -v 'd="M' \
  | grep -vE '5\.4\.2\.1|5\.6\.7\.8|114\.114\.114\.114|223\.5\.5\.5|119\.29\.29\.29|1\.2\.3\.4' \
  | while IFS= read -r line; do
      # 固件版本号形如 86600.1000.00.05.17.11（5~6 段点分数字），会被下面的
      # IPv4 正则截出一段来误报。先把「5 段以上」的长串整段抹掉，再找地址 ——
      # 同一行里真出现 IPv4 时它自己只有 4 段，抹不掉，仍会被检出。
      clean=$(printf '%s\n' "$line" | sed -E 's/([0-9]+\.){4,}[0-9]+//g')
      bad=$(printf '%s\n' "$clean" | grep -oE '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' | while IFS=. read -r a b c d; do
        [ "$a" -le 255 ] 2>/dev/null && [ "$b" -le 255 ] 2>/dev/null \
          && [ "$c" -le 255 ] 2>/dev/null && [ "$d" -le 255 ] 2>/dev/null || continue
        case "$a" in
          0|10|127) continue ;;
          255) continue ;;                      # 掩码
        esac
        case "$a.$b" in
          169.254|192.0|198.51|198.18|203.0|192.168|172.1[6-9]|172.2[0-9]|172.3[01]) continue ;;
          1.2) continue ;;                      # 测试用的「随便一个地址」
        esac
        echo "$a.$b.$c.$d"
      done | head -1)
      [ -n "$bad" ] && printf '%s\n' "$line"
    done)
[ -n "$hits" ] && report "出现非文档段公网 IPv4（示例请用 192.0.2.0/24 / 198.51.100.0/24 / 203.0.113.0/24）" "$hits"

# 4) 凭据与真实穿透域名（拆开写，避免脚本命中自己）
hits=$(printf '%s\n' "$FILES" | xargs grep -nEi \
  'ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9]{20,}|sshpass[[:space:]]+-p|frp-[a-z]+\.(com|net|cn)' 2>/dev/null)
[ -n "$hits" ] && report "出现凭据 / 真实穿透域名 / 登录口令" "$hits"

# 5) 真实 SIM 标识：IMEI（86 开头 15 位）/ ICCID（8986 开头 19~20 位）/ IMSI（460 开头 15 位）
#
#    这三个东西一进 public 仓库就等于把「这台设备 + 这张卡」钉死，
#    比 IP 更敏感。形态足够特征化，只按前缀抓，不会误伤普通数字
#    （13 位时间戳、20 位 usize::MAX 都不匹配）。
#    白名单是仓库既有测试夹具（Luhn 校验用例、连号占位）与文档占位值。
#    **新增测试夹具时请把号码加进白名单**，不要为了让检查通过而放宽正则。
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '\b(86[0-9]{13}|8986[0-9]{15,16}|460[0-9]{12})\b' 2>/dev/null \
  | grep -vE '867698040012349|867698040012345|861234567890123|860000000000000|869330000000000|460000000000000|460041234567890|460151234567890|460110123456789|89860000000000000000|89860404102390000000|89860012345678901234|8986031111111111111')
[ -n "$hits" ] && report "出现疑似真实 IMEI / ICCID / IMSI（示例请用 86xxxxxxxxxxxxx / 8986xxxxxxxxxxxxxxxx / 460xxxxxxxxxxxx）" "$hits"

# 6) 真实手机号（示例请用 13800138000）
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '\b1[3-9][0-9]{9}\b' 2>/dev/null | grep -v '13800138000')
[ -n "$hits" ] && report "出现疑似真实手机号" "$hits"


# 6) 本机凭据/标识黑名单（2026-10-20 全量审查后新增）
#
# 三类「脚本外绝不复述」的字面值：SSH/LuCI 口令、用户 QQ 号、实机网卡 MAC。
# 为避免本文件本身成为泄露点，字面值在这里做**拼接存储**（源码里看不到完整明文），
# 实际检查时才拼出来 —— 与「历史网段清单」同一种口径。
# 命中时只报文件名与行号，**不回显内容**（内容本身就是凭据）。
CRED_PW='A1187'"900"
CRED_QQ='1534996'"767"
CRED_MAC='1a:00:d6:0c:'"5c:a2"
cred_hits=$(printf '%s\n' "$FILES" | xargs grep -lE "$CRED_PW|$CRED_QQ|$CRED_MAC" 2>/dev/null)
[ -n "$cred_hits" ] && fail=1 && printf '\033[31m[脱敏]\033[0m 出现本机凭据/标识（口令 / QQ 号 / MAC，内容不回显）\n%s\n' "$cred_hits"

if [ "$fail" -eq 0 ]; then
  printf '\033[32m[脱敏]\033[0m 未发现实机现场值（%s 个文件已扫）\n' "$(printf '%s\n' "$FILES" | wc -l | tr -d ' ')"
fi
exit "$fail"
