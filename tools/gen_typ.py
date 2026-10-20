# style/gsi.typ.txt を生成するスクリプト。TYP を変更するときはこちらを編集して再生成する:
#   python3 tools/gen_typ.py style/gsi.typ.txt
import sys
out = []
w = out.append
w('; -*- coding: UTF-8 -*-')
w('; 地理院地図Vector 風の TYP 定義（mkgmap の TYP コンパイラ用テキスト形式）')
w('; tools/gen_typ.py で生成している。直接編集しないこと。')
w('; 色は地理院地図（ラスタの標準地図）に合わせている。FID/CodePage は mkgmap のオプションから設定される。')
w('')
w('[_drawOrder]')
w('Type=0x03c,1')
w('Type=0x013,2')
w('[end]')
w('')

def polygon(t, name, color):
    w('[_polygon]'); w(f'Type={t}'); w(f'String={name}')
    w('Xpm="0 0 1 0"'); w(f'"a c {color}"'); w('[end]'); w('')

def line_solid(t, name, color, width, border=None, label=None):
    w('[_line]'); w(f'Type={t}'); w(f'String={name}'); w('UseOrientation=N')
    w(f'LineWidth={width}')
    if border:
        w('BorderWidth=1'); w('Xpm="0 0 2 0"'); w(f'"a c {color}"'); w(f'"b c {border}"')
    else:
        w('Xpm="0 0 1 0"'); w(f'"a c {color}"')
    font(label); w('[end]'); w('')

def line_pattern(t, name, color, rows, label=None, bg='none'):
    """32 x 線幅 のビットマップで描く線。'#' が color、'.' が bg（既定は透明）。"""
    assert all(len(r) == 32 for r in rows), (t, [len(r) for r in rows])
    w('[_line]'); w(f'Type={t}'); w(f'String={name}'); w('UseOrientation=Y')
    w(f'Xpm="32 {len(rows)} 2 1"'); w(f'"# c {color}"'); w(f'". c {bg}"')
    for r in rows: w(f'"{r}"')
    font(label); w('[end]'); w('')

def font(label):
    if label is None:
        w('FontStyle=NoLabel')
    else:
        style, color = label
        w(f'FontStyle={style}')
        if color: w(f'DayCustomColor={color}')

def point(t, name, rows=None, color='#000000', label=('SmallFont', '#000000')):
    if rows is None:
        rows = ['.']  # 文字だけの注記: 透明な 1x1 アイコン
    wd = len(rows[0]); assert all(len(r) == wd for r in rows), (t, name)
    w('[_point]'); w(f'Type={t}'); w(f'String={name}')
    used = set(''.join(rows))
    if used == {'.'}:
        w(f'DayXpm="{wd} {len(rows)} 1 1"'); w('". c none"')
    else:
        w(f'DayXpm="{wd} {len(rows)} 2 1"'); w(f'"# c {color}"'); w('". c none"')
    for r in rows: w(f'"{r}"')
    font(label); w('[end]'); w('')

CONTOUR = '#C8A03C'; CONTOUR_TEXT = '#A07820'; GRAY = '#646464'; WATER = '#145AFF'; BOUNDARY = '#440080'
# 駅名・路線名・道路名などの交通関係の注記の色
TRANSPORT = '#136145'

# ---- 面 ----
polygon('0x3c', '水域', '#BED2FF')
# 地理院地図の建物は淡い塗りに橙の縁取り。Garmin の面は縁取りできないので中間の色で塗る
polygon('0x13', '建物', '#FFB48C')

# ---- 等高線 ----
line_pattern('0x20', '補助曲線', CONTOUR, ['#' * 26 + '.' * 6], ('SmallFont', CONTOUR_TEXT))
line_solid('0x21', '主曲線', CONTOUR, 1, label=('SmallFont', CONTOUR_TEXT))
line_solid('0x22', '計曲線', CONTOUR, 2, label=('SmallFont', CONTOUR_TEXT))

# ---- 道路 ----
line_solid('0x01', '高速道路', '#64C373', 4, GRAY)
line_solid('0x02', '国道', '#FF7A7A', 4, GRAY)
line_solid('0x03', '都道府県道', '#FFFF00', 4, GRAY)
line_solid('0x05', '道路（5.5m以上）', '#FFFFFF', 3, GRAY)
line_solid('0x06', '道路（3m-5.5m）', '#FFFFFF', 2, GRAY)
line_solid('0x07', '道路（3m未満）', GRAY, 1)
# 徒歩道: 地理院地図では灰色の破線。登山で見落とさないよう地理院より濃くしている
line_pattern('0x16', '徒歩道', '#505050', ['##########......' * 2] * 2)

# ---- 鉄道・河川・境界・構造物 ----
# JR は地理院地図と同じく灰色の縁に白と灰の交互の破線（旗竿）
line_pattern('0x14', 'JR', GRAY, ['#' * 32, '########........' * 2, '########........' * 2, '#' * 32], bg='#FFFFFF')
line_solid('0x10e04', '鉄道（JR以外）', GRAY, 3)
line_pattern('0x10e05', '鉄道（トンネル）', GRAY, ['######....' * 3 + '##'] * 2)
line_solid('0x18', '河川', WATER, 2)
line_pattern('0x26', '河川（枯れ川部）', WATER, ['##########..' * 2 + '##########'[:8]] * 2)
line_pattern('0x1d', '都府県界', BOUNDARY, ['##############..##..............'] * 2)
line_pattern('0x1c', '市区町村界', BOUNDARY, ['############..#..#..############'[:32]] * 2)
line_solid('0x29', '送電線', GRAY, 1)

# ---- 地形表現（拡張種別）----
line_solid('0x10e00', '岩・崖', CONTOUR, 2)
line_solid('0x10e01', '雨裂', CONTOUR, 3)
line_solid('0x10e02', '滝', GRAY, 3)

# ---- 注記（文字だけ）----
point('0x6616', '山名', label=('NormalFont', '#511D0A'))
point('0x660b', '峠', label=('SmallFont', '#000000'))
point('0x6508', '滝', label=('SmallFont', '#000000'))
point('0x650d', '水部の名称', label=('SmallFont', WATER))
point('0x0800', '市区町村', label=('LargeFont', '#000000'))
point('0x0b00', '町字名', label=('NormalFont', '#000000'))
point('0x0c00', '集落名', label=('SmallFont', '#505050'))
point('0x2f08', '駅', label=('NormalFont', TRANSPORT))
point('0x2f09', '鉄道路線名', label=('SmallFont', TRANSPORT))
point('0x2f0a', '道路名・道路施設', label=('SmallFont', TRANSPORT))
point('0x2800', '注記', label=('SmallFont', '#000000'))

# ---- 地図記号 ----
# 標高点・三角点は山名より文字を優先させないため 0x66xx の後ろの番号にしている（style/gsi/points を参照）
point('0x661e', '標高点', ['###', '###', '###'])
point('0x661f', '三角点', [
    '....#....',
    '...#.#...',
    '...#.#...',
    '..#...#..',
    '..#.#.#..',
    '.#.....#.',
    '.#.....#.',
    '#########'])
point('0x2803', '水準点', [
    '#######',
    '#.....#',
    '#.....#',
    '#..#..#',
    '#.....#',
    '#.....#',
    '#######'], label=None)
point('0x2805', '神社', [
    '###########',
    '.#########.',
    '...#...#...',
    '###########',
    '...#...#...',
    '...#...#...',
    '...#...#...',
    '...#...#...',
    '...#...#...'], label=None)
point('0x2806', '寺院', [
    '#...#####',
    '#...#....',
    '#...#....',
    '#...#....',
    '#########',
    '....#...#',
    '....#...#',
    '....#...#',
    '#####...#'], label=None)
point('0x2807', '温泉', [
    '..#..#..#..',
    '.#..#..#...',
    '..#..#..#..',
    '...#..#..#.',
    '..#..#..#..',
    '#.........#',
    '#.........#',
    '#.........#',
    '.#.......#.',
    '..#.....#..',
    '...#####...'], color='#C80000', label=None)
point('0x2808', '郵便局', [
    '...#####...',
    '..#.....#..',
    '.#.......#.',
    '#.#######.#',
    '#.........#',
    '#.#######.#',
    '#....#....#',
    '.#...#...#.',
    '..#..#..#..',
    '...#####...'], label=None)
point('0x2809', '病院', [
    '.#######.',
    '#.......#',
    '#...#...#',
    '#...#...#',
    '#.#####.#',
    '#...#...#',
    '#...#...#',
    '.#.....#.',
    '..#####..'], label=None)
point('0x280a', '学校', [
    '....#....',
    '#########',
    '.#.....#.',
    '..#...#..',
    '...#.#...',
    '....#....',
    '...#.#...',
    '..#...#..',
    '##.....##'], label=None)
point('0x280b', '記念碑', [
    '..###..',
    '..#.#..',
    '..#.#..',
    '..#.#..',
    '..#.#..',
    '.#...#.',
    '.#...#.',
    '#######',
    '#######'], label=None)
point('0x280c', '警察', [
    '#.....#',
    '.#...#.',
    '..#.#..',
    '...#...',
    '..#.#..',
    '.#...#.',
    '#.....#'], label=None)
point('0x280e', '役所・役場', [
    '..#####..',
    '.#.....#.',
    '#.......#',
    '#.......#',
    '#.......#',
    '#.......#',
    '#.......#',
    '.#.....#.',
    '..#####..'], label=None)
point('0x280f', '史跡・名勝', [
    '...##....',
    '...##....',
    '.........',
    '##....##.',
    '##....##.'], label=None)

open(sys.argv[1], 'w', encoding='utf-8').write('\n'.join(out) + '\n')
