// flclash-api is a minimal, stdlib-only bridge that exposes the FlClash
// WebUI contract (/flclash/api/*) on top of a local mihomo controller and
// serves the static WebUI from the same origin.
//
// It deliberately has no writes outside:
//   - <data>/config.yaml and <data>/profiles/*.yaml (profile management)
//   - the core init script (start/stop/restart)
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	maxHistory = 60
	maxLogs    = 500
	maxBody    = 32 << 20
)

type logEntry struct {
	Time    int64  `json:"time"`
	Level   string `json:"level"`
	Message string `json:"message"`
}

type profile struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Type      string `json:"type"`
	URL       string `json:"url"`
	UpdatedAt int64  `json:"updatedAt"`
	Active    bool   `json:"active"`
}

type Bridge struct {
	controller string
	uiDir      string
	dataDir    string
	initScript string
	secret     string

	client       *http.Client
	streamClient *http.Client

	mu       sync.Mutex
	histUp   []float64
	histDown []float64
	peakUp   float64
	peakDown float64
	totalUp  float64
	totalDn  float64
	logs     []logEntry
	started  time.Time
}

func main() {
	var (
		listen     = flag.String("listen", "0.0.0.0:9098", "listen address")
		controller = flag.String("controller", "127.0.0.1:9097", "mihomo external controller")
		uiDir      = flag.String("ui", "/www/flclash", "static WebUI directory")
		dataDir    = flag.String("data", "/etc/flclash", "config and profile directory")
		initScript = flag.String("init", "/etc/init.d/flclash-core", "core init script")
	)
	flag.Parse()

	b := &Bridge{
		controller:   strings.TrimPrefix(strings.TrimPrefix(*controller, "http://"), "https://"),
		uiDir:        *uiDir,
		dataDir:      *dataDir,
		initScript:   *initScript,
		secret:       os.Getenv("FLCLASH_API_SECRET"),
		client:       &http.Client{Timeout: 10 * time.Second},
		streamClient: &http.Client{},
		started:      time.Now(),
	}

	go b.sampleTraffic()
	go b.sampleLogs()
	go b.sampleTotals()

	mux := http.NewServeMux()
	mux.HandleFunc("/flclash/api/", b.handleAPI)
	mux.HandleFunc("/flclash/api", b.handleAPI)
	mux.Handle("/flclash/", http.StripPrefix("/flclash/", http.FileServer(http.Dir(b.uiDir))))
	mux.HandleFunc("/flclash", func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "/flclash/", http.StatusFound)
	})
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "/flclash/", http.StatusFound)
	})

	log.Printf("flclash-api listening on %s, controller %s, ui %s", *listen, b.controller, b.uiDir)
	log.Fatal(http.ListenAndServe(*listen, mux))
}

/* ------------------------------------------------------------------ helpers */

func writeJSON(w http.ResponseWriter, code int, v interface{}) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func fail(w http.ResponseWriter, code int, err error) {
	writeJSON(w, code, map[string]string{"error": err.Error()})
}

func urlPath(s string) string  { return url.PathEscape(s) }
func urlQuery(s string) string { return url.QueryEscape(s) }

func (b *Bridge) authorized(r *http.Request) bool {
	if b.secret == "" {
		return true
	}
	got := r.Header.Get("X-FlClash-Secret")
	if got == "" {
		got = strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
	}
	return got == b.secret
}

func (b *Bridge) api(method, path string, body interface{}) ([]byte, int, error) {
	var reader io.Reader
	if body != nil {
		raw, err := json.Marshal(body)
		if err != nil {
			return nil, 0, err
		}
		reader = strings.NewReader(string(raw))
	}
	req, err := http.NewRequest(method, "http://"+b.controller+path, reader)
	if err != nil {
		return nil, 0, err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if b.secret != "" {
		req.Header.Set("Authorization", "Bearer "+b.secret)
	}
	resp, err := b.client.Do(req)
	if err != nil {
		return nil, 0, err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxBody))
	if err != nil {
		return nil, resp.StatusCode, err
	}
	if resp.StatusCode >= 400 {
		return data, resp.StatusCode, fmt.Errorf("controller %s %s: %s", method, path, strings.TrimSpace(string(data)))
	}
	return data, resp.StatusCode, nil
}

func (b *Bridge) apiJSON(method, path string, body interface{}, out interface{}) error {
	data, _, err := b.api(method, path, body)
	if err != nil {
		return err
	}
	return json.Unmarshal(data, out)
}

/* ----------------------------------------------------------- samplers (API) */

func (b *Bridge) stream(path string, fn func(map[string]interface{})) {
	for {
		req, err := http.NewRequest("GET", "http://"+b.controller+path, nil)
		if err != nil {
			time.Sleep(2 * time.Second)
			continue
		}
		if b.secret != "" {
			req.Header.Set("Authorization", "Bearer "+b.secret)
		}
		resp, err := b.streamClient.Do(req)
		if err != nil {
			time.Sleep(2 * time.Second)
			continue
		}
		dec := json.NewDecoder(resp.Body)
		for {
			var msg map[string]interface{}
			if err := dec.Decode(&msg); err != nil {
				break
			}
			fn(msg)
		}
		resp.Body.Close()
		time.Sleep(time.Second)
	}
}

func (b *Bridge) sampleTraffic() {
	b.stream("/traffic", func(msg map[string]interface{}) {
		up, _ := msg["up"].(float64)
		down, _ := msg["down"].(float64)

		b.mu.Lock()
		defer b.mu.Unlock()
		b.histUp = append(b.histUp, up)
		b.histDown = append(b.histDown, down)
		if len(b.histUp) > maxHistory {
			b.histUp = b.histUp[len(b.histUp)-maxHistory:]
		}
		if len(b.histDown) > maxHistory {
			b.histDown = b.histDown[len(b.histDown)-maxHistory:]
		}
		if up > b.peakUp {
			b.peakUp = up
		}
		if down > b.peakDown {
			b.peakDown = down
		}
	})
}

func (b *Bridge) sampleLogs() {
	b.stream("/logs?level=info", func(msg map[string]interface{}) {
		payload, _ := msg["payload"].(string)
		level, _ := msg["type"].(string)
		if payload == "" {
			return
		}
		if level == "" {
			level = "info"
		}
		b.mu.Lock()
		defer b.mu.Unlock()
		b.logs = append(b.logs, logEntry{Time: time.Now().Unix(), Level: level, Message: payload})
		if len(b.logs) > maxLogs {
			b.logs = b.logs[len(b.logs)-maxLogs:]
		}
	})
}

func (b *Bridge) sampleTotals() {
	for {
		var payload struct {
			DownloadTotal float64 `json:"downloadTotal"`
			UploadTotal   float64 `json:"uploadTotal"`
		}
		if err := b.apiJSON("GET", "/connections", nil, &payload); err == nil {
			b.mu.Lock()
			b.totalUp = payload.UploadTotal
			b.totalDn = payload.DownloadTotal
			b.mu.Unlock()
		}
		time.Sleep(3 * time.Second)
	}
}

/* ------------------------------------------------------------ system facts */

func readFileTrim(path string) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(data))
}

func (b *Bridge) corePID() int {
	entries, err := os.ReadDir("/proc")
	if err != nil {
		return 0
	}
	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		pid, err := strconv.Atoi(entry.Name())
		if err != nil {
			continue
		}
		if readFileTrim(filepath.Join("/proc", entry.Name(), "comm")) == "mihomo" {
			return pid
		}
	}
	return 0
}

func rssKB(path string) float64 {
	data, err := os.ReadFile(path)
	if err != nil {
		return 0
	}
	for _, line := range strings.Split(string(data), "\n") {
		if strings.HasPrefix(line, "VmRSS:") {
			fields := strings.Fields(line)
			if len(fields) >= 2 {
				kb, _ := strconv.ParseFloat(fields[1], 64)
				return kb * 1024
			}
		}
	}
	return 0
}

func (b *Bridge) coreUptime() float64 {
	pid := b.corePID()
	if pid == 0 {
		return 0
	}
	stat := readFileTrim(filepath.Join("/proc", strconv.Itoa(pid), "stat"))
	idx := strings.LastIndex(stat, ")")
	if idx < 0 {
		return 0
	}
	fields := strings.Fields(stat[idx+1:])
	/* field 22 (starttime) is index 19 after the comm field. */
	if len(fields) < 20 {
		return 0
	}
	startTicks, _ := strconv.ParseFloat(fields[19], 64)
	upLine := readFileTrim("/proc/uptime")
	upFields := strings.Fields(upLine)
	if len(upFields) == 0 {
		return 0
	}
	upSec, _ := strconv.ParseFloat(upFields[0], 64)
	value := upSec - startTicks/100.0
	if value < 0 {
		return 0
	}
	return value
}

func localIPv4() string {
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		return ""
	}
	for _, addr := range addrs {
		ipnet, ok := addr.(*net.IPNet)
		if !ok || ipnet.IP.IsLoopback() {
			continue
		}
		ip := ipnet.IP.To4()
		if ip == nil {
			continue
		}
		return ip.String()
	}
	return ""
}

/* --------------------------------------------------------------- API models */

type mihomoConfig struct {
	Port        int    `json:"port"`
	SocksPort   int    `json:"socks-port"`
	MixedPort   int    `json:"mixed-port"`
	AllowLan    bool   `json:"allow-lan"`
	Mode        string `json:"mode"`
	LogLevel    string `json:"log-level"`
	IPv6        bool   `json:"ipv6"`
	Tun         struct {
		Enable bool `json:"enable"`
	} `json:"tun"`
}

type mihomoProxy struct {
	Type    string `json:"type"`
	Now     string `json:"now"`
	All     []string `json:"all"`
	History []struct {
		Delay int `json:"delay"`
	} `json:"history"`
}

type mihomoProxies struct {
	Proxies map[string]mihomoProxy `json:"proxies"`
}

type mihomoConnection struct {
	ID       string `json:"id"`
	Upload   int64  `json:"upload"`
	Download int64  `json:"download"`
	Start    string `json:"start"`
	Chains   []string `json:"chains"`
	Rule     string `json:"rule"`
	Metadata struct {
		Host            string `json:"host"`
		DestinationIP   string `json:"destinationIP"`
		DestinationPort string `json:"destinationPort"`
		SourceIP        string `json:"sourceIP"`
		Network         string `json:"network"`
		ProcessPath     string `json:"processPath"`
	} `json:"metadata"`
}

type mihomoConnections struct {
	DownloadTotal float64            `json:"downloadTotal"`
	UploadTotal   float64            `json:"uploadTotal"`
	Connections   []mihomoConnection `json:"connections"`
}

func proxyDelay(p mihomoProxy) int {
	if len(p.History) == 0 {
		return 0
	}
	return p.History[len(p.History)-1].Delay
}

func (b *Bridge) readConfig() (mihomoConfig, error) {
	var cfg mihomoConfig
	err := b.apiJSON("GET", "/configs", nil, &cfg)
	return cfg, err
}

func (b *Bridge) readProxies() (mihomoProxies, error) {
	var data mihomoProxies
	err := b.apiJSON("GET", "/proxies", nil, &data)
	return data, err
}

func (b *Bridge) readConnections() (mihomoConnections, error) {
	var data mihomoConnections
	err := b.apiJSON("GET", "/connections", nil, &data)
	return data, err
}

/* ------------------------------------------------------------------- status */

func (b *Bridge) status() map[string]interface{} {
	cfg, cfgErr := b.readConfig()
	proxies, _ := b.readProxies()
	conns, _ := b.readConnections()
	version := map[string]interface{}{}
	_ = b.apiJSON("GET", "/version", nil, &version)

	groups := make([]map[string]interface{}, 0, 8)
	timeout := 0
	for name, p := range proxies.Proxies {
		if len(p.All) == 0 {
			continue
		}
		groups = append(groups, map[string]interface{}{
			"name":  name,
			"type":  strings.ToLower(p.Type),
			"now":   p.Now,
			"count": len(p.All),
		})
		if proxyDelay(p) == 0 {
			timeout++
		}
	}
	sort.Slice(groups, func(i, j int) bool {
		return groups[i]["name"].(string) < groups[j]["name"].(string)
	})

	b.mu.Lock()
	histUp := append([]float64{}, b.histUp...)
	histDown := append([]float64{}, b.histDown...)
	peakUp, peakDown := b.peakUp, b.peakDown
	totalUp, totalDn := b.totalUp, b.totalDn
	b.mu.Unlock()

	upRate, downRate := 0.0, 0.0
	if len(histUp) > 0 {
		upRate = histUp[len(histUp)-1]
	}
	if len(histDown) > 0 {
		downRate = histDown[len(histDown)-1]
	}

	pid := b.corePID()
	coreRSS := 0.0
	if pid != 0 {
		coreRSS = rssKB(filepath.Join("/proc", strconv.Itoa(pid), "status"))
	}

	active := b.activeProfile()

	return map[string]interface{}{
		"running":      cfgErr == nil,
		"version":      version["version"],
		"coreVersion":  version["version"],
		"uptime":       b.coreUptime(),
		"mode":         cfg.Mode,
		"outboundIp":   "",
		"countryCode":  "",
		"organization": "",
		"quality": map[string]string{
			"type":  "未知",
			"level": "质量未知",
		},
		"localIp":     localIPv4(),
		"mixedPort":   cfg.MixedPort,
		"socksPort":   cfg.SocksPort,
		"httpPort":    cfg.Port,
		"systemProxy": false,
		"tun":         cfg.Tun.Enable,
		"allowLan":    cfg.AllowLan,
		"ipv6Enabled": cfg.IPv6,
		"logLevel":    cfg.LogLevel,
		"traffic": map[string]interface{}{
			"up": upRate, "down": downRate,
			"upRate": upRate, "downRate": downRate,
			"peakUp": peakUp, "peakDown": peakDown,
			"totalUp": totalUp, "totalDown": totalDn,
			"history": map[string]interface{}{"up": histUp, "down": histDown},
		},
		"memory": map[string]interface{}{
			"app":  rssKB("/proc/self/status"),
			"core": coreRSS,
		},
		"services": []interface{}{},
		"stats": map[string]interface{}{
			"connections": len(conns.Connections),
			"requests":    0,
			"dns":         0,
		},
		"profile": active,
		"proxyGroups": map[string]interface{}{
			"count":   len(groups),
			"timeout": timeout,
			"groups":  groups,
		},
	}
}

/* ------------------------------------------------------------------ proxies */

func (b *Bridge) proxiesPayload() (map[string]interface{}, error) {
	data, err := b.readProxies()
	if err != nil {
		return nil, err
	}

	groups := make([]map[string]interface{}, 0, 8)
	nodes := make(map[string]map[string]interface{})

	for name, p := range data.Proxies {
		if len(p.All) == 0 {
			continue
		}
		groups = append(groups, map[string]interface{}{
			"name":  name,
			"type":  strings.ToLower(p.Type),
			"now":   p.Now,
			"count": len(p.All),
		})
		for _, member := range p.All {
			item, ok := data.Proxies[member]
			if !ok {
				continue
			}
			if existing, ok := nodes[member]; ok {
				existing["groups"] = append(existing["groups"].([]string), name)
				continue
			}
			nodes[member] = map[string]interface{}{
				"name":   member,
				"type":   strings.ToLower(item.Type),
				"delay":  proxyDelay(item),
				"group":  name,
				"groups": []string{name},
			}
		}
	}

	sort.Slice(groups, func(i, j int) bool {
		return groups[i]["name"].(string) < groups[j]["name"].(string)
	})

	list := make([]map[string]interface{}, 0, len(nodes))
	for _, node := range nodes {
		list = append(list, node)
	}
	sort.Slice(list, func(i, j int) bool {
		return list[i]["name"].(string) < list[j]["name"].(string)
	})

	return map[string]interface{}{"groups": groups, "nodes": list}, nil
}

/* -------------------------------------------------------------- connections */

func (b *Bridge) connectionsPayload() []map[string]interface{} {
	data, err := b.readConnections()
	if err != nil {
		return []map[string]interface{}{}
	}

	out := make([]map[string]interface{}, 0, len(data.Connections))
	now := time.Now()
	for _, c := range data.Connections {
		chain := ""
		if len(c.Chains) > 0 {
			chain = c.Chains[0]
		}
		port, _ := strconv.Atoi(c.Metadata.DestinationPort)
		start, err := time.Parse(time.RFC3339Nano, c.Start)
		duration := 0.0
		if err == nil {
			duration = now.Sub(start).Seconds()
		}
		host := c.Metadata.Host
		if host == "" {
			host = c.Metadata.DestinationIP
		}
		out = append(out, map[string]interface{}{
			"id":         c.ID,
			"host":       host,
			"port":       port,
			"rule":       c.Rule,
			"chain":      chain,
			"up":         c.Upload,
			"down":       c.Download,
			"startTime":  start.Unix() * 1000,
			"duration":   duration,
			"source":     c.Metadata.SourceIP,
			"network":    c.Metadata.Network,
			"process":    filepath.Base(c.Metadata.ProcessPath),
		})
	}
	return out
}

/* ------------------------------------------------------------------- config */

func (b *Bridge) configPayload() map[string]interface{} {
	cfg, _ := b.readConfig()
	return map[string]interface{}{
		"mode":        cfg.Mode,
		"mixedPort":   cfg.MixedPort,
		"socksPort":   cfg.SocksPort,
		"httpPort":    cfg.Port,
		"allowLan":    cfg.AllowLan,
		"ipv6":        cfg.IPv6,
		"tun":         cfg.Tun.Enable,
		"logLevel":    cfg.LogLevel,
		"systemProxy": false,
	}
}

func (b *Bridge) saveConfig(patch map[string]interface{}) error {
	next := map[string]interface{}{}
	if v, ok := patch["mode"].(string); ok {
		next["mode"] = v
	}
	if v, ok := patch["logLevel"].(string); ok {
		next["log-level"] = v
	}
	if v, ok := patch["mixedPort"].(float64); ok {
		next["mixed-port"] = int(v)
	}
	if v, ok := patch["allowLan"].(bool); ok {
		next["allow-lan"] = v
	}
	if v, ok := patch["ipv6"].(bool); ok {
		next["ipv6"] = v
	}
	if v, ok := patch["tun"].(bool); ok {
		next["tun"] = map[string]interface{}{"enable": v}
	}
	if len(next) == 0 {
		return nil
	}
	_, _, err := b.api("PATCH", "/configs", next)
	return err
}

/* ----------------------------------------------------------------- profiles */

func (b *Bridge) indexPath() string    { return filepath.Join(b.dataDir, "index.json") }
func (b *Bridge) profilesDir() string { return filepath.Join(b.dataDir, "profiles") }
func (b *Bridge) configPath() string  { return filepath.Join(b.dataDir, "config.yaml") }

func (b *Bridge) loadProfiles() []profile {
	var list []profile
	data, err := os.ReadFile(b.indexPath())
	if err == nil {
		_ = json.Unmarshal(data, &list)
	}
	return list
}

func (b *Bridge) saveProfiles(list []profile) error {
	if err := os.MkdirAll(b.profilesDir(), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(list, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(b.indexPath(), data, 0o644)
}

func (b *Bridge) activeProfile() map[string]interface{} {
	list := b.loadProfiles()
	for _, item := range list {
		if item.Active {
			return map[string]interface{}{
				"id": item.ID, "name": item.Name, "type": "file",
				"url": item.URL, "updatedAt": humanAge(item.UpdatedAt),
				"expireDays": 0, "usedShare": 0,
				"used": "0 B", "total": "0 B",
				"proxies": 0, "groups": 0, "rules": 0, "active": true,
			}
		}
	}
	return map[string]interface{}{}
}

func humanAge(ts int64) string {
	if ts == 0 {
		return "—"
	}
	delta := time.Since(time.Unix(ts, 0))
	switch {
	case delta < time.Minute:
		return "刚刚"
	case delta < time.Hour:
		return fmt.Sprintf("%d 分钟前", int(delta.Minutes()))
	case delta < 24*time.Hour:
		return fmt.Sprintf("%d 小时前", int(delta.Hours()))
	default:
		return fmt.Sprintf("%d 天前", int(delta.Hours()/24))
	}
}

func (b *Bridge) importProfile(url string) (profile, error) {
	name := strings.TrimSuffix(filepath.Base(strings.Split(url, "?")[0]), filepath.Ext(url))
	if name == "" || name == "." || name == "/" {
		name = "订阅"
	}
	id := fmt.Sprintf("p%d", time.Now().UnixNano())

	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return profile{}, err
	}
	client := &http.Client{Timeout: 45 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return profile{}, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return profile{}, fmt.Errorf("下载配置失败: HTTP %d", resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxBody))
	if err != nil {
		return profile{}, err
	}
	if len(body) == 0 {
		return profile{}, fmt.Errorf("下载到的配置为空")
	}

	if err := os.MkdirAll(b.profilesDir(), 0o755); err != nil {
		return profile{}, err
	}
	if err := os.WriteFile(filepath.Join(b.profilesDir(), id+".yaml"), body, 0o600); err != nil {
		return profile{}, err
	}

	item := profile{ID: id, Name: name, Type: "url", URL: url, UpdatedAt: time.Now().Unix()}
	b.mu.Lock()
	list := b.loadProfiles()
	list = append(list, item)
	err = b.saveProfiles(list)
	b.mu.Unlock()
	return item, err
}

func (b *Bridge) applyProfile(id string) error {
	src := filepath.Join(b.profilesDir(), id+".yaml")
	data, err := os.ReadFile(src)
	if err != nil {
		return fmt.Errorf("配置不存在: %s", id)
	}
	if err := os.MkdirAll(b.dataDir, 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(b.configPath(), data, 0o600); err != nil {
		return err
	}

	b.mu.Lock()
	list := b.loadProfiles()
	for i := range list {
		list[i].Active = list[i].ID == id
	}
	err = b.saveProfiles(list)
	b.mu.Unlock()
	if err != nil {
		return err
	}

	/* Ask the running core to reload; fall back to a service restart. */
	if _, _, err := b.api("PUT", "/configs", map[string]string{"path": b.configPath()}); err != nil {
		_ = b.service("restart")
	}
	return nil
}

func (b *Bridge) deleteProfile(id string) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	list := b.loadProfiles()
	next := make([]profile, 0, len(list))
	for _, item := range list {
		if item.ID != id {
			next = append(next, item)
		}
	}
	if err := b.saveProfiles(next); err != nil {
		return err
	}
	_ = os.Remove(filepath.Join(b.profilesDir(), id+".yaml"))
	return nil
}

/* ------------------------------------------------------------------ service */

func (b *Bridge) service(action string) error {
	switch action {
	case "start", "stop", "restart":
	default:
		return fmt.Errorf("未知操作: %s", action)
	}
	cmd := exec.Command(b.initScript, action)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("%s: %s", err, strings.TrimSpace(string(out)))
	}
	return nil
}

/* ------------------------------------------------------------------ routing */

func (b *Bridge) handleAPI(w http.ResponseWriter, r *http.Request) {
	if !b.authorized(r) {
		fail(w, http.StatusUnauthorized, fmt.Errorf("unauthorized"))
		return
	}

	path := strings.TrimPrefix(r.URL.Path, "/flclash/api")
	if path == "" {
		path = "/"
	}

	switch {
	case path == "/status" && r.Method == http.MethodGet:
		writeJSON(w, http.StatusOK, b.status())

	case path == "/version" && r.Method == http.MethodGet:
		data, _, err := b.api("GET", "/version", nil)
		if err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, json.RawMessage(data))

	case path == "/memory" && r.Method == http.MethodGet:
		pid := b.corePID()
		core := 0.0
		if pid != 0 {
			core = rssKB(filepath.Join("/proc", strconv.Itoa(pid), "status"))
		}
		app := rssKB("/proc/self/status")
		writeJSON(w, http.StatusOK, map[string]interface{}{
			"rss": app + core, "app": app, "core": core,
		})

	case path == "/config" && r.Method == http.MethodGet:
		writeJSON(w, http.StatusOK, b.configPayload())

	case path == "/config" && (r.Method == http.MethodPut || r.Method == http.MethodPatch):
		var patch map[string]interface{}
		if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&patch); err != nil {
			fail(w, http.StatusBadRequest, err)
			return
		}
		if err := b.saveConfig(patch); err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]interface{}{"ok": true})

	case path == "/proxies" && r.Method == http.MethodGet:
		payload, err := b.proxiesPayload()
		if err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, payload)

	case path == "/proxies/select" && r.Method == http.MethodPost:
		var body struct {
			Group string `json:"group"`
			Name  string `json:"name"`
		}
		if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&body); err != nil {
			fail(w, http.StatusBadRequest, err)
			return
		}
		if body.Group == "" || body.Name == "" {
			fail(w, http.StatusBadRequest, fmt.Errorf("group and name are required"))
			return
		}
		if _, _, err := b.api("PUT", "/proxies/"+urlPath(body.Group), map[string]string{"name": body.Name}); err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]interface{}{"ok": true})

	case path == "/proxies/delay" && r.Method == http.MethodPost:
		var body struct {
			Name string `json:"name"`
		}
		if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&body); err != nil {
			fail(w, http.StatusBadRequest, err)
			return
		}
		query := "/proxies/" + urlPath(body.Name) + "/delay?timeout=5000&url=" + urlQuery("https://www.gstatic.com/generate_204")
		data, _, err := b.api("GET", query, nil)
		if err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		var out struct {
			Delay int `json:"delay"`
		}
		_ = json.Unmarshal(data, &out)
		writeJSON(w, http.StatusOK, map[string]interface{}{"name": body.Name, "value": out.Delay})

	case path == "/connections" && r.Method == http.MethodGet:
		writeJSON(w, http.StatusOK, b.connectionsPayload())

	case strings.HasPrefix(path, "/connections/") && r.Method == http.MethodDelete:
		id := strings.TrimPrefix(path, "/connections/")
		if id == "" {
			fail(w, http.StatusBadRequest, fmt.Errorf("missing id"))
			return
		}
		if _, _, err := b.api("DELETE", "/connections/"+urlPath(id), nil); err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]interface{}{"ok": true})

	case path == "/dns/queries" && r.Method == http.MethodGet:
		writeJSON(w, http.StatusOK, []interface{}{})

	case path == "/dns/flush" && r.Method == http.MethodPost:
		_, _, _ = b.api("POST", "/cache/fakeip/flush", nil)
		writeJSON(w, http.StatusOK, map[string]interface{}{"ok": true})

	case path == "/logs" && r.Method == http.MethodGet:
		b.mu.Lock()
		out := append([]logEntry{}, b.logs...)
		b.mu.Unlock()
		writeJSON(w, http.StatusOK, out)

	case path == "/system/proxy" && r.Method == http.MethodGet:
		cfg, _ := b.readConfig()
		writeJSON(w, http.StatusOK, map[string]interface{}{
			"backend":         "flclash",
			"mixed_port":      cfg.MixedPort,
			"clash_api_port":  9097,
			"tun_name":        "",
			"routing_mode":    cfg.Mode,
			"ipv6_support":    cfg.IPv6,
			"running":         true,
			"read_only":       true,
			"source":          "flclash-api",
		})

	case path == "/profiles" && r.Method == http.MethodGet:
		list := b.loadProfiles()
		out := make([]map[string]interface{}, 0, len(list))
		for _, item := range list {
			out = append(out, map[string]interface{}{
				"id": item.ID, "name": item.Name, "type": item.Type, "url": item.URL,
				"updatedAt": humanAge(item.UpdatedAt), "expireDays": 0, "usedShare": 0,
				"used": "0 B", "total": "0 B", "proxies": 0, "groups": 0, "rules": 0,
				"active": item.Active,
			})
		}
		writeJSON(w, http.StatusOK, out)

	case path == "/profiles/import" && r.Method == http.MethodPost:
		var body struct {
			URL string `json:"url"`
		}
		if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&body); err != nil {
			fail(w, http.StatusBadRequest, err)
			return
		}
		if body.URL == "" {
			fail(w, http.StatusBadRequest, fmt.Errorf("url is required"))
			return
		}
		item, err := b.importProfile(body.URL)
		if err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, item)

	case strings.HasPrefix(path, "/profiles/") && strings.HasSuffix(path, "/apply") && r.Method == http.MethodPost:
		id := strings.TrimSuffix(strings.TrimPrefix(path, "/profiles/"), "/apply")
		if err := b.applyProfile(id); err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]interface{}{"ok": true, "id": id})

	case strings.HasPrefix(path, "/profiles/") && r.Method == http.MethodDelete:
		id := strings.TrimPrefix(path, "/profiles/")
		if err := b.deleteProfile(id); err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]interface{}{"ok": true, "id": id})

	case strings.HasPrefix(path, "/service/") && r.Method == http.MethodPost:
		action := strings.TrimPrefix(path, "/service/")
		if err := b.service(action); err != nil {
			fail(w, http.StatusBadGateway, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]interface{}{"ok": true, "action": action})

	default:
		fail(w, http.StatusNotFound, fmt.Errorf("not found: %s", path))
	}
}
