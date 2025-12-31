package ui

import (
	"fmt"
	"net/url"
	"strings"
	"time"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/isalikov/radio-record-cli/internal/api"
	"github.com/isalikov/radio-record-cli/internal/config"
	"github.com/isalikov/radio-record-cli/internal/player"
)

var (
	titleStyle = lipgloss.NewStyle().
			Bold(true).
			Foreground(lipgloss.Color("#FF6600"))

	selectedStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FF6600")).
			Bold(true)

	normalStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FFFFFF"))

	dimStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#666666"))

	matchStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FFFF00"))

	favoriteStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FF69B4"))

	genreStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#00FFFF"))

	nowPlayingStyle = lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(lipgloss.Color("#FF6600")).
			Padding(0, 1)

	searchStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FFFFFF")).
			Background(lipgloss.Color("#333333"))

	helpStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#666666"))

	volumeStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#00FF00"))

	statusBarStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FFFFFF")).
			Background(lipgloss.Color("#333333"))

	tabActiveStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#000000")).
			Background(lipgloss.Color("#FF6600")).
			Padding(0, 1)

	tabInactiveStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#888888")).
			Padding(0, 1)
)

type mode int

const (
	modeNormal mode = iota
	modeSearch
	modeHelp
)

type Model struct {
	stations      []api.Station
	visibleList   []int
	allGenres     []string
	currentGenre  int
	filtered      []int
	cursor        int
	selected      int
	player        *player.Player
	client        *api.Client
	config        *config.Config
	nowPlaying    *api.Track
	err           error
	width         int
	height        int
	loading       bool
	mode          mode
	searchQuery   string
	matchIndex    int
	showFavorites bool
}

type stationsLoadedMsg struct {
	stations []api.Station
	err      error
}

type nowPlayingMsg struct {
	track *api.Track
}

type tickMsg time.Time

func NewModel(client *api.Client, p *player.Player, cfg *config.Config) Model {
	return Model{
		client:       client,
		player:       p,
		config:       cfg,
		selected:     -1,
		loading:      true,
		mode:         modeNormal,
		filtered:     []int{},
		visibleList:  []int{},
		currentGenre: -1,
		width:        80,
		height:       24,
	}
}

func (m Model) Init() tea.Cmd {
	return tea.Batch(
		loadStations(m.client),
		tickCmd(),
	)
}

func loadStations(client *api.Client) tea.Cmd {
	return func() tea.Msg {
		stations, err := client.GetStations()
		return stationsLoadedMsg{stations: stations, err: err}
	}
}

func fetchNowPlaying(client *api.Client, stationID int) tea.Cmd {
	return func() tea.Msg {
		track, _ := client.GetNowPlaying(stationID)
		return nowPlayingMsg{track: track}
	}
}

func tickCmd() tea.Cmd {
	return tea.Tick(5*time.Second, func(t time.Time) tea.Msg {
		return tickMsg(t)
	})
}

func (m *Model) extractGenres() {
	genreMap := make(map[string]bool)
	for _, s := range m.stations {
		for _, g := range s.Genres {
			genreMap[g.Name] = true
		}
	}
	m.allGenres = []string{}
	for g := range genreMap {
		m.allGenres = append(m.allGenres, g)
	}
}

func (m *Model) updateVisibleList() {
	m.visibleList = []int{}

	// На вкладке "Все" (currentGenre == -1) избранные станции идут первыми
	if m.currentGenre == -1 && !m.showFavorites {
		// Сначала добавляем избранные
		for i, s := range m.stations {
			if m.config.IsFavorite(s.ID) {
				m.visibleList = append(m.visibleList, i)
			}
		}
		// Затем остальные
		for i, s := range m.stations {
			if !m.config.IsFavorite(s.ID) {
				m.visibleList = append(m.visibleList, i)
			}
		}
	} else {
		// Для других вкладок — стандартная логика
		for i, s := range m.stations {
			if m.showFavorites && !m.config.IsFavorite(s.ID) {
				continue
			}

			if m.currentGenre >= 0 && m.currentGenre < len(m.allGenres) {
				genreName := m.allGenres[m.currentGenre]
				hasGenre := false
				for _, g := range s.Genres {
					if g.Name == genreName {
						hasGenre = true
						break
					}
				}
				if !hasGenre {
					continue
				}
			}

			m.visibleList = append(m.visibleList, i)
		}
	}

	if m.cursor >= len(m.visibleList) {
		m.cursor = 0
	}
}

func (m *Model) doSearch() {
	m.filtered = []int{}
	if m.searchQuery == "" {
		return
	}

	query := strings.ToLower(m.searchQuery)
	for _, idx := range m.visibleList {
		s := m.stations[idx]
		title := strings.ToLower(s.Title)
		tooltip := strings.ToLower(s.Tooltip)
		if strings.Contains(title, query) || strings.Contains(tooltip, query) {
			m.filtered = append(m.filtered, idx)
		}
	}

	if len(m.filtered) > 0 {
		m.matchIndex = 0
		for i, idx := range m.visibleList {
			if idx == m.filtered[0] {
				m.cursor = i
				break
			}
		}
	}
}

func (m *Model) nextMatch() {
	if len(m.filtered) == 0 {
		return
	}
	m.matchIndex = (m.matchIndex + 1) % len(m.filtered)
	for i, idx := range m.visibleList {
		if idx == m.filtered[m.matchIndex] {
			m.cursor = i
			break
		}
	}
}

func (m *Model) prevMatch() {
	if len(m.filtered) == 0 {
		return
	}
	m.matchIndex--
	if m.matchIndex < 0 {
		m.matchIndex = len(m.filtered) - 1
	}
	for i, idx := range m.visibleList {
		if idx == m.filtered[m.matchIndex] {
			m.cursor = i
			break
		}
	}
}

func (m *Model) isMatch(stationIdx int) bool {
	for _, i := range m.filtered {
		if i == stationIdx {
			return true
		}
	}
	return false
}

func (m *Model) clearSearch() {
	m.searchQuery = ""
	m.filtered = []int{}
	m.matchIndex = 0
}

// highlightMatch подсвечивает вхождения query в text
func highlightMatch(text, query string, hlStyle lipgloss.Style) string {
	if query == "" {
		return text
	}

	textLower := strings.ToLower(text)
	queryLower := strings.ToLower(query)

	var result strings.Builder
	lastEnd := 0

	for {
		idx := strings.Index(textLower[lastEnd:], queryLower)
		if idx == -1 {
			result.WriteString(text[lastEnd:])
			break
		}

		start := lastEnd + idx
		end := start + len(query)

		// Добавляем текст до совпадения
		result.WriteString(text[lastEnd:start])
		// Добавляем подсвеченное совпадение
		result.WriteString(hlStyle.Render(text[start:end]))

		lastEnd = end
	}

	return result.String()
}

func (m *Model) playStation(stationIdx int) tea.Cmd {
	if stationIdx < 0 || stationIdx >= len(m.stations) {
		return nil
	}
	m.selected = stationIdx
	station := m.stations[stationIdx]
	m.player.Play(station.Stream320)
	return fetchNowPlaying(m.client, station.ID)
}

func (m *Model) getStationAtCursor() int {
	if m.cursor >= 0 && m.cursor < len(m.visibleList) {
		return m.visibleList[m.cursor]
	}
	return -1
}

func (m *Model) listHeight() int {
	// header: 2, tabs: 1, separator: 1, status: 2, now playing: 7 (track + 3 links + borders), footer: 1
	reserved := 14
	if m.selected < 0 || m.nowPlaying == nil {
		reserved = 7
	}
	h := m.height - reserved
	if h < 5 {
		h = 5
	}
	return h
}

func (m Model) renderTabs() string {
	var tabs []string

	// "Все" tab
	if m.currentGenre == -1 {
		tabs = append(tabs, tabActiveStyle.Render("Все"))
	} else {
		tabs = append(tabs, tabInactiveStyle.Render("Все"))
	}

	// Genre tabs
	for i, genre := range m.allGenres {
		if i == m.currentGenre {
			tabs = append(tabs, tabActiveStyle.Render(genre))
		} else {
			tabs = append(tabs, tabInactiveStyle.Render(genre))
		}
	}

	// Join tabs and handle overflow
	tabsLine := strings.Join(tabs, " ")

	// If tabs are too long, show scrollable view centered on current
	tabsWidth := lipgloss.Width(tabsLine)
	if tabsWidth > m.width {
		// Calculate visible range around current genre
		var visibleTabs []string
		currentIdx := m.currentGenre + 1 // +1 because "Все" is index 0

		// Build tabs with limited visibility
		startIdx := currentIdx - 3
		if startIdx < 0 {
			startIdx = 0
		}

		endIdx := startIdx + 7
		if endIdx > len(m.allGenres)+1 {
			endIdx = len(m.allGenres) + 1
			startIdx = endIdx - 7
			if startIdx < 0 {
				startIdx = 0
			}
		}

		if startIdx > 0 {
			visibleTabs = append(visibleTabs, dimStyle.Render("◀"))
		}

		for i := startIdx; i < endIdx && i <= len(m.allGenres); i++ {
			if i == 0 {
				if m.currentGenre == -1 {
					visibleTabs = append(visibleTabs, tabActiveStyle.Render("Все"))
				} else {
					visibleTabs = append(visibleTabs, tabInactiveStyle.Render("Все"))
				}
			} else {
				genre := m.allGenres[i-1]
				if i-1 == m.currentGenre {
					visibleTabs = append(visibleTabs, tabActiveStyle.Render(genre))
				} else {
					visibleTabs = append(visibleTabs, tabInactiveStyle.Render(genre))
				}
			}
		}

		if endIdx <= len(m.allGenres) {
			visibleTabs = append(visibleTabs, dimStyle.Render("▶"))
		}

		tabsLine = strings.Join(visibleTabs, " ")
	}

	return tabsLine
}

func (m Model) renderHelp() string {
	helpBox := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(lipgloss.Color("#FF6600")).
		Padding(1, 2).
		Width(m.width - 4)

	title := titleStyle.Render("📻 Radio Record CLI — Справка")

	help := `
  Навигация                         Воспроизведение
  ─────────────────────────────     ─────────────────────────────
  j / ↓         Вниз                Enter / Space Играть станцию
  k / ↑         Вверх               s             Остановить
  g             В начало списка     + / =         Громкость +5
  G             В конец списка      - / _         Громкость -5

  Поиск (vim-style)                 Фильтры
  ─────────────────────────────     ─────────────────────────────
  /             Начать поиск        Tab           Следующий жанр
  Enter         Применить поиск     Shift+Tab     Предыдущий жанр
  Esc           Отменить/сбросить   f             В избранное
  n             След. совпадение    F             Только избранное
  N             Пред. совпадение

  Быстрый доступ                    Прочее
  ─────────────────────────────     ─────────────────────────────
  0             Сбросить фильтры    ?             Показать справку
  1-9           Избранное #1-9      q / Ctrl+C    Выход`

	footer := dimStyle.Render("\n  Нажми любую клавишу для выхода...")

	content := title + "\n\n" + helpBox.Render(help) + footer

	// Center vertically
	lines := strings.Count(content, "\n") + 1
	topPadding := (m.height - lines) / 2
	if topPadding < 0 {
		topPadding = 0
	}

	return strings.Repeat("\n", topPadding) + content
}

func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		if m.mode == modeHelp {
			m.mode = modeNormal
			return m, nil
		}

		if m.mode == modeSearch {
			switch msg.String() {
			case "enter":
				m.mode = modeNormal
				m.searchQuery = "" // Убираем подсветку при выходе из поиска
			case "esc":
				m.mode = modeNormal
				m.clearSearch()
			case "backspace":
				if len(m.searchQuery) > 0 {
					// Корректное удаление UTF-8 символа (включая кириллицу)
					runes := []rune(m.searchQuery)
					m.searchQuery = string(runes[:len(runes)-1])
					m.doSearch()
				}
			default:
				if utf8.RuneCountInString(msg.String()) == 1 {
					m.searchQuery += msg.String()
					m.doSearch()
				}
			}
			return m, nil
		}

		switch msg.String() {
		case "ctrl+c", "q":
			m.player.Stop()
			return m, tea.Quit

		case "/":
			m.mode = modeSearch
			m.searchQuery = ""

		case "?":
			m.mode = modeHelp

		case "esc":
			m.clearSearch()

		case "n":
			m.nextMatch()

		case "N":
			m.prevMatch()

		case "up", "k":
			if m.cursor > 0 {
				m.cursor--
			}

		case "down", "j":
			if m.cursor < len(m.visibleList)-1 {
				m.cursor++
			}

		case "g":
			m.cursor = 0

		case "G":
			if len(m.visibleList) > 0 {
				m.cursor = len(m.visibleList) - 1
			}

		case "enter", " ":
			stationIdx := m.getStationAtCursor()
			if stationIdx >= 0 {
				return m, m.playStation(stationIdx)
			}

		case "s":
			m.player.Stop()
			m.selected = -1
			m.nowPlaying = nil

		case "+", "=":
			m.player.VolumeUp()

		case "-", "_":
			m.player.VolumeDown()

		case "tab":
			m.currentGenre++
			if m.currentGenre >= len(m.allGenres) {
				m.currentGenre = -1
			}
			m.updateVisibleList()
			m.clearSearch()

		case "shift+tab":
			m.currentGenre--
			if m.currentGenre < -1 {
				m.currentGenre = len(m.allGenres) - 1
			}
			m.updateVisibleList()
			m.clearSearch()

		case "f":
			stationIdx := m.getStationAtCursor()
			if stationIdx >= 0 {
				m.config.ToggleFavorite(m.stations[stationIdx].ID)
				// Обновляем список если в режиме избранного или на вкладке "Все" (где избранные вверху)
				if m.showFavorites || m.currentGenre == -1 {
					m.updateVisibleList()
				}
			}

		case "F":
			m.showFavorites = !m.showFavorites
			m.updateVisibleList()
			m.clearSearch()

		case "0":
			m.currentGenre = -1
			m.showFavorites = false
			m.updateVisibleList()
			m.clearSearch()

		case "1", "2", "3", "4", "5", "6", "7", "8", "9":
			idx := int(msg.String()[0] - '1')
			if idx < len(m.config.Favorites) {
				favID := m.config.Favorites[idx]
				for i, s := range m.stations {
					if s.ID == favID {
						return m, m.playStation(i)
					}
				}
			}
		}

	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height

	case stationsLoadedMsg:
		m.loading = false
		if msg.err != nil {
			m.err = msg.err
		} else {
			m.stations = msg.stations
			m.extractGenres()
			m.updateVisibleList()
		}

	case nowPlayingMsg:
		m.nowPlaying = msg.track

	case tickMsg:
		cmds := []tea.Cmd{tickCmd()}
		if m.selected >= 0 && m.selected < len(m.stations) {
			cmds = append(cmds, fetchNowPlaying(m.client, m.stations[m.selected].ID))
		}
		return m, tea.Batch(cmds...)
	}

	return m, nil
}

func (m Model) View() string {
	if m.loading {
		// Center loading message
		msg := "⏳ Загрузка станций..."
		topPad := m.height / 2
		leftPad := (m.width - len(msg)) / 2
		if leftPad < 0 {
			leftPad = 0
		}
		return strings.Repeat("\n", topPad) + strings.Repeat(" ", leftPad) + titleStyle.Render(msg)
	}

	if m.err != nil {
		return fmt.Sprintf("Ошибка: %v", m.err)
	}

	if m.mode == modeHelp {
		return m.renderHelp()
	}

	var sections []string

	// === HEADER ===
	title := "📻 Radio Record CLI"
	if m.showFavorites {
		title += " " + favoriteStyle.Render("[♥ Избранное]")
	}

	// Volume on the right
	vol := m.player.Volume()
	volStr := volumeStyle.Render(fmt.Sprintf("%d%%", vol))

	titleLen := lipgloss.Width(title)
	volLen := lipgloss.Width(volStr)
	spacer := m.width - titleLen - volLen - 2
	if spacer < 1 {
		spacer = 1
	}

	header := titleStyle.Render(title) + strings.Repeat(" ", spacer) + volStr
	sections = append(sections, header)

	// === TABS ===
	sections = append(sections, m.renderTabs())
	sections = append(sections, strings.Repeat("─", m.width))

	// === STATION LIST ===
	listHeight := m.listHeight()

	start := 0
	if m.cursor >= listHeight {
		start = m.cursor - listHeight + 1
	}

	end := start + listHeight
	if end > len(m.visibleList) {
		end = len(m.visibleList)
	}

	var listLines []string

	if len(m.visibleList) == 0 {
		emptyMsg := "  Нет станций для отображения"
		listLines = append(listLines, dimStyle.Render(emptyMsg))
	}

	for i := start; i < end; i++ {
		stationIdx := m.visibleList[i]
		station := m.stations[stationIdx]

		cursor := "  "
		style := normalStyle

		if i == m.cursor {
			cursor = "▸ "
			style = selectedStyle
		}

		if stationIdx == m.selected {
			cursor = "♪ "
			style = selectedStyle
		}

		if m.isMatch(stationIdx) && i != m.cursor {
			style = matchStyle
		}

		favMark := ""
		if m.config.IsFavorite(station.ID) {
			favMark = favoriteStyle.Render("♥ ")
		}

		hotkey := ""
		for idx, favID := range m.config.Favorites {
			if favID == station.ID && idx < 9 {
				hotkey = dimStyle.Render(fmt.Sprintf("[%d] ", idx+1))
				break
			}
		}

		// Номер станции (индекс из API, 1-based)
		stationNum := dimStyle.Render(fmt.Sprintf("%3d. ", stationIdx+1))

		// Truncate tooltip if needed
		maxTitleLen := 20
		maxTooltipLen := m.width - maxTitleLen - 20 // уменьшаем для номера станции
		if maxTooltipLen < 10 {
			maxTooltipLen = 10
		}

		tooltip := station.Tooltip
		if len(tooltip) > maxTooltipLen {
			tooltip = tooltip[:maxTooltipLen-3] + "..."
		}

		// Подсветка совпадений при поиске
		title := station.Title
		if m.searchQuery != "" && m.isMatch(stationIdx) {
			title = highlightMatch(station.Title, m.searchQuery, matchStyle)
			tooltip = highlightMatch(tooltip, m.searchQuery, matchStyle)
		}

		line := fmt.Sprintf("%s%s%s%s%-*s %s", cursor, stationNum, hotkey, favMark, maxTitleLen, title, dimStyle.Render(tooltip))
		listLines = append(listLines, style.Render(line))
	}

	// Pad list to fixed height
	for len(listLines) < listHeight {
		listLines = append(listLines, "")
	}

	sections = append(sections, strings.Join(listLines, "\n"))

	// === STATUS BAR ===
	sections = append(sections, strings.Repeat("─", m.width))

	info := fmt.Sprintf(" %d/%d станций", m.cursor+1, len(m.visibleList))
	if len(m.filtered) > 0 {
		info += fmt.Sprintf(" │ Поиск: %d/%d", m.matchIndex+1, len(m.filtered))
	}

	// Pad status bar to full width
	infoLen := lipgloss.Width(info)
	if infoLen < m.width {
		info += strings.Repeat(" ", m.width-infoLen)
	}
	sections = append(sections, statusBarStyle.Render(info))

	// === NOW PLAYING ===
	if m.selected >= 0 && m.nowPlaying != nil {
		artist := m.nowPlaying.Artist
		song := m.nowPlaying.Song
		if artist == "" {
			artist = "Radio Record"
		}

		npContent := fmt.Sprintf("▶ %s — %s", artist, song)

		// Truncate if too long
		maxNpLen := m.width - 6
		if len(npContent) > maxNpLen {
			npContent = npContent[:maxNpLen-3] + "..."
		}

		// Music service links
		query := url.QueryEscape(artist + " " + song)
		ytLink := fmt.Sprintf("https://music.youtube.com/search?q=%s", query)
		yaLink := fmt.Sprintf("https://music.yandex.ru/search?text=%s", query)
		spLink := fmt.Sprintf("https://open.spotify.com/search/%s", query)

		linksLine := dimStyle.Render(fmt.Sprintf("YT Music: %s", ytLink))
		linksLine2 := dimStyle.Render(fmt.Sprintf("Yandex:   %s", yaLink))
		linksLine3 := dimStyle.Render(fmt.Sprintf("Spotify:  %s", spLink))

		npBox := npContent + "\n" + linksLine + "\n" + linksLine2 + "\n" + linksLine3
		np := nowPlayingStyle.Width(m.width - 4).Render(npBox)
		sections = append(sections, np)
	}

	// === FOOTER ===
	var footer string
	if m.mode == modeSearch {
		searchLine := fmt.Sprintf("/%s▌", m.searchQuery)
		footer = searchStyle.Width(m.width).Render(searchLine)
	} else {
		helpText := "? справка │ / поиск │ ←Tab→ жанры │ 0 сброс │ f ♥ │ +/- 🔊 │ Enter ▶"
		footerPad := (m.width - lipgloss.Width(helpText)) / 2
		if footerPad < 0 {
			footerPad = 0
		}
		footer = helpStyle.Render(strings.Repeat(" ", footerPad) + helpText)
	}
	sections = append(sections, footer)

	return strings.Join(sections, "\n")
}
