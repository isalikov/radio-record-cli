//! System media controls belong to the application, independent of its audio engine.
#[cfg(target_os = "macos")]
mod macos {
    use crate::{
        app::App,
        error::{Error, Result},
        player::PlayerState,
    };
    use block2::RcBlock;
    use crossterm::event::{KeyCode, MediaKeyCode};
    use objc2::{
        MainThreadMarker,
        rc::{Retained, autoreleasepool},
        runtime::AnyObject,
    };
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::{
        NSDate, NSDefaultRunLoopMode, NSDictionary, NSNumber, NSRunLoop, NSString,
    };
    use objc2_media_player::{
        MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingInfoPropertyIsLiveStream,
        MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter,
        MPRemoteCommandHandlerStatus,
    };
    use std::sync::mpsc::{self, Receiver};

    pub struct MediaControls {
        _application: Retained<NSApplication>,
        center: Retained<MPNowPlayingInfoCenter>,
        targets: Vec<(Retained<MPRemoteCommand>, Retained<AnyObject>)>,
        commands: Receiver<KeyCode>,
        last: Option<(String, PlayerState)>,
    }

    impl MediaControls {
        pub fn new() -> Result<Self> {
            let main = MainThreadMarker::new()
                .ok_or_else(|| Error::new("Media controls require the main thread"))?;
            let application = NSApplication::sharedApplication(main);
            application.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
            application.finishLaunching();
            let (tx, commands) = mpsc::channel();
            // All Cocoa setup, metadata and teardown run on the main thread. The
            // framework copies the blocks; callbacks only enqueue application input.
            unsafe {
                let remote = MPRemoteCommandCenter::sharedCommandCenter();
                let mut targets = Vec::new();
                for (command, key) in [
                    (remote.previousTrackCommand(), MediaKeyCode::TrackPrevious),
                    (remote.togglePlayPauseCommand(), MediaKeyCode::PlayPause),
                    (remote.nextTrackCommand(), MediaKeyCode::TrackNext),
                    (remote.playCommand(), MediaKeyCode::Play),
                    (remote.pauseCommand(), MediaKeyCode::Pause),
                ] {
                    let tx = tx.clone();
                    let handler = RcBlock::new(move |_| {
                        if tx.send(KeyCode::Media(key)).is_ok() {
                            MPRemoteCommandHandlerStatus::Success
                        } else {
                            MPRemoteCommandHandlerStatus::CommandFailed
                        }
                    });
                    let target = command.addTargetWithHandler(&handler);
                    command.setEnabled(false);
                    targets.push((command, target));
                }
                Ok(Self {
                    _application: application,
                    center: MPNowPlayingInfoCenter::defaultCenter(),
                    targets,
                    commands,
                    last: None,
                })
            }
        }

        pub fn poll(&self) -> Vec<KeyCode> {
            // Cocoa delivers remote commands through the main run loop. Pump it
            // without waiting; crossterm remains responsible for the TUI's pacing.
            autoreleasepool(|_| {
                let run_loop = NSRunLoop::mainRunLoop();
                let deadline = NSDate::distantPast();
                for _ in 0..8 {
                    if !run_loop.runMode_beforeDate(unsafe { NSDefaultRunLoopMode }, &deadline) {
                        break;
                    }
                }
            });
            self.commands.try_iter().collect()
        }

        pub fn update(&mut self, app: &App) {
            let next = app.now.as_ref().and_then(|station| {
                matches!(
                    app.playback.state,
                    PlayerState::Playing | PlayerState::Paused | PlayerState::Buffering
                )
                .then(|| (station.title.clone(), app.playback.state))
            });
            if self.last == next {
                return;
            }
            autoreleasepool(|_| unsafe {
                for (command, _) in &self.targets {
                    command.setEnabled(next.is_some());
                }
                if let Some((title, state)) = &next {
                    let title = NSString::from_str(title);
                    let live = NSNumber::new_bool(true);
                    let values: [&AnyObject; 2] = [&title, &live];
                    let info = NSDictionary::from_slices(
                        &[
                            MPMediaItemPropertyTitle,
                            MPNowPlayingInfoPropertyIsLiveStream,
                        ],
                        &values,
                    );
                    self.center.setNowPlayingInfo(Some(&info));
                    self.center
                        .setPlaybackState(if *state == PlayerState::Paused {
                            MPNowPlayingPlaybackState::Paused
                        } else {
                            MPNowPlayingPlaybackState::Playing
                        });
                } else {
                    self.center
                        .setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                    self.center.setNowPlayingInfo(None);
                }
            });
            self.last = next;
        }
    }

    impl Drop for MediaControls {
        fn drop(&mut self) {
            unsafe {
                for (command, target) in &self.targets {
                    command.removeTarget(Some(target));
                    command.setEnabled(false);
                }
                self.center
                    .setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                self.center.setNowPlayingInfo(None);
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::MediaControls;

#[cfg(not(target_os = "macos"))]
pub struct MediaControls;

#[cfg(not(target_os = "macos"))]
impl MediaControls {
    pub fn new() -> crate::error::Result<Self> {
        Ok(Self)
    }
    pub fn poll(&self) -> Vec<crossterm::event::KeyCode> {
        Vec::new()
    }
    pub fn update(&mut self, _: &crate::app::App) {}
}
