# Funkstrom - General Overview

Welcome to Funkstrom! This guide will help you understand what Funkstrom is and what you can do with it, without getting too technical.

## What is Funkstrom?

Funkstrom is your personal internet radio station. It takes your music collection and streams it continuously over your network, just like a real radio station. You can listen to it from any device - your phone, computer, smart speakers, or any device that can play internet radio.

Think of it as running your own Spotify or Pandora, but with complete control over what plays and when.

## Why Use Funkstrom?

- **Your Music, Your Way**: Play your own music collection instead of relying on streaming services
- **Multiple Quality Options**: Offer different audio qualities for different devices and network speeds
- **Scheduled Programming**: Create a real radio experience with shows at specific times
- **Listen Everywhere**: Stream to any device on your network
- **Always On**: Your radio station runs 24/7, ready whenever you want to listen
- **Discovery**: Integrate with hearthis.at to discover and stream new electronic music sets

## What Can You Do With It?

### Basic Use Cases

**Home Music Server**
Stream your music collection to any device in your home. Perfect for background music throughout the day.

**Personal Radio Station**
Create a radio station experience with scheduled shows. Play energetic music in the morning, chill music in the evening, or special shows on weekends.

**Multi-Device Streaming**
Listen to the same station on multiple devices simultaneously. Everyone hears the same thing at the same time, just like a real radio station.

**Mobile & Desktop Listening**
Offer different quality streams for mobile devices (lower bandwidth) and desktop computers (higher quality).

**Music Discovery**
Schedule live DJ sets from hearthis.at to discover new electronic music automatically.

## Key Concepts

### Your Music Library

This is your collection of audio files stored on your computer. Funkstrom scans this folder and indexes all your music. You can choose to:
- **Shuffle**: Play tracks in random order (great for variety)
- **Sequential**: Play tracks in order (good for albums)
- **Repeat**: Loop back to the beginning when all tracks have played (keep the music going 24/7)

Supported formats include MP3, FLAC, OGG, M4A, WAV, and many more.

### Streams

A stream is how your music gets delivered to listeners. Think of each stream as a different radio channel with its own quality settings.

You can create multiple streams for different purposes:

- **High Quality** (320 kbps): For desktop listeners with good internet connections
- **Standard** (128 kbps): For most everyday listening
- **Mobile** (48-64 kbps): For smartphones or slower connections

All streams play the same content at the same time - they just differ in audio quality and bandwidth usage.

**Example Stream URLs:**
- `http://your-server:8284/high` - High quality stream
- `http://your-server:8284/standard` - Standard quality stream
- `http://your-server:8284/mobile` - Mobile-optimized stream

### Programs & Scheduling

Programs let you schedule specific content to play at certain times, creating a real radio station experience.

**Two Types of Programs:**

1. **Playlist Programs**
   Play specific tracks from a playlist file at scheduled times.

   Example: Play your "Morning Energy" playlist every weekday at 7 AM for 3 hours.

2. **Liveset Programs**
   Automatically fetch and stream DJ sets from hearthis.at based on genres you choose.

   Example: Stream techno sets every Friday night at 10 PM for 4 hours.

Programs use cron-style scheduling, which sounds technical but is actually simple:
- `0 7 * * 1-5` = 7:00 AM, Monday through Friday
- `0 22 * * 5,6` = 10:00 PM, Friday and Saturday
- `0 14 * * 0` = 2:00 PM, Sunday

When a program isn't running, your regular music library plays instead.

### How Broadcasting Works

Unlike services like Spotify where everyone can pause and play independently, Funkstrom works like a real radio station:

- **Everyone hears the same thing**: All listeners connected to a stream hear the same track at the same point in time
- **No pause/rewind**: You can't pause or go back - it's a continuous live stream
- **Join anytime**: When you connect, you hear whatever is currently playing
- **Always broadcasting**: The music plays continuously whether anyone is listening or not

This creates a shared listening experience, just like tuning into a real radio station.

## Getting Started

### Basic Setup

1. **Install Funkstrom** on your computer or server
2. **Create a configuration file** (`config.toml`) with:
   - Where your music is stored
   - What port to use (like 8284)
   - Your station name and description
   - At least one stream configuration
3. **Start the server**
4. **Listen** by opening the stream URL in any media player

### First Run

The first time you start Funkstrom, it will scan your entire music library. This can take a few minutes for large collections:
- 1,000 tracks: ~30 seconds
- 10,000 tracks: ~5 minutes
- 50,000 tracks: ~20-30 minutes

After the first scan, starting up is much faster (just seconds) because it only checks for changes.

## Listening to Your Station

You can listen using many different applications:

### Media Players
- **VLC**: Open Network Stream → Enter `http://your-server:8284/standard`
- **mpv**: Run `mpv http://your-server:8284/standard`
- **iTunes/Apple Music**: File → Open Stream
- **Winamp**: Add URL to playlist

### Web Browser
Just open `http://your-server:8284/` to see your station's info page with an embedded player and links to all streams.

### Mobile Apps
- **iOS**: VLC for Mobile, foobar2000 mobile
- **Android**: VLC, Simple Radio, TuneIn (via custom URL)

### Smart Speakers
Some smart speakers can play custom internet radio URLs through their apps.

## Understanding Your Station

### The Web Interface

Open `http://your-server:8284/` in a browser to see:

- **Station Information**: Your station name, description, and genre
- **Now Playing**: The current track with artist and album info
- **Available Streams**: Links to all your configured streams with their quality levels
- **Built-in Player**: Play directly in your browser

### Monitoring

Several endpoints let you check what's happening:

**`/status`** - Shows:
- All available streams and their status (online/offline)
- Buffer information (how much audio is queued)
- Station metadata

**`/current`** - Shows:
- Currently playing track details
- Artist, album, and title information
- File path

You can check these in your browser or use tools like `curl` to integrate with other systems.

## Real-World Scenarios

### Scenario 1: Simple Home Radio

**Goal**: Stream your music collection around your home.

**Setup**:
- One standard quality stream (128 kbps)
- Shuffle enabled
- Repeat enabled (music never stops)

**Result**: 24/7 background music you can tune into from any device.

---

### Scenario 2: Quality Options for Different Devices

**Goal**: Offer multiple quality levels for different situations.

**Setup**:
- High quality stream (320 kbps) for desktop listening
- Standard stream (128 kbps) for general use
- Mobile stream (48 kbps) for smartphones on cellular data

**Result**: Everyone gets the best quality for their situation and bandwidth.

---

### Scenario 3: Radio Station with Shows

**Goal**: Create a radio station experience with scheduled programming.

**Setup**:
- Regular library plays most of the time
- Morning show: Upbeat playlist at 7 AM weekdays (3 hours)
- Evening chill: Ambient playlist at 7 PM daily (2 hours)
- Weekend special: Techno livesets from hearthis.at on Friday/Saturday nights (4 hours)

**Result**: A dynamic listening experience that changes throughout the day and week.

---

### Scenario 4: Electronic Music Discovery Station

**Goal**: Automatically play fresh DJ sets from hearthis.at.

**Setup**:
- Schedule multiple liveset programs throughout the day
- Different genres for different times
- Morning: Ambient/Chill
- Afternoon: Deep House
- Evening: Techno/Techhouse

**Result**: Always fresh content without manually curating playlists.

## Tips for Best Experience

### Music Library Organization

- **Use good metadata**: Ensure your music files have proper ID3 tags (title, artist, album)
- **Consistent format**: While Funkstrom supports many formats, consistency makes management easier
- **Organized folders**: While not required, organizing by artist/album makes it easier to create playlists

### Stream Configuration

- **Start with one stream**: Get comfortable before adding multiple quality options
- **Match your network**: Use lower bitrates if you have limited upload bandwidth
- **Consider your audience**: Who will listen and from where?

### Scheduling

- **Test first**: Try your scheduled programs manually before setting them up to run automatically
- **Mind the duration**: Make sure program durations make sense (don't schedule 4 hours of a 30-minute playlist)
- **Overlap handling**: If programs overlap, the most recently started one takes over

### Performance

- **Dedicated server**: For best reliability, run on a dedicated machine or server
- **Storage space**: The audio buffer uses memory - not much, but keep it in mind for very long uptimes
- **FFmpeg dependency**: Ensure FFmpeg is installed and working before starting

## Common Questions

**Q: Can I pause or rewind the stream?**
A: No, Funkstrom is a live broadcast. Everyone hears the same thing at the same time, like a real radio station.

**Q: What happens if I disconnect and reconnect?**
A: You'll rejoin the stream at the current position. You won't hear what you missed while disconnected.

**Q: Can different people listen to different parts of my library?**
A: No, everyone listening to the same stream hears the same broadcast. If you want different content for different people, run multiple instances of Funkstrom.

**Q: How many people can listen at once?**
A: There's no built-in limit, but your network bandwidth is the practical limitation. Each listener uses bandwidth equal to the stream bitrate.

**Q: Can I access this over the internet?**
A: Yes, but you'll need to configure your router/firewall to forward the port, or use a reverse proxy. Be aware of bandwidth usage.

**Q: What if my music library changes?**
A: Restart Funkstrom and it will automatically detect new, changed, or removed files.

**Q: Can I see who's listening?**
A: The `/status` endpoint shows the number of connected listeners, but not who they are.

**Q: Do scheduled programs interrupt the current track?**
A: Yes, when a program starts, it begins immediately, even if a track is playing.

## What's Next?

Now that you understand the basics, you can:

1. **Check the Configuration Guide** (`configuration.md`) for detailed setup instructions
2. **Explore the example config** (`config.toml.example`) to see all available options
3. **Experiment with scheduling** to create your perfect radio experience
4. **Try different stream qualities** to find the right balance for your setup

## Getting Help

If you run into issues:

- Check the logs (set `RUST_LOG=info` or `RUST_LOG=debug`)
- Review the configuration documentation
- Check that FFmpeg is installed and accessible
- Verify your music directory is readable
- Ensure the port isn't already in use

---

Funkstrom gives you the power to run your own radio station with complete control. Whether you want simple background music or a sophisticated scheduled programming experience, it's all possible. Enjoy your broadcasting!
