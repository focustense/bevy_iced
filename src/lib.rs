//! # Use Iced UI programs in your Bevy application
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_iced::iced::widget::text;
//! use bevy_iced::{IcedContext, IcedPlugin};
//!
//! pub enum UiMessage {}
//!
//! pub fn main() {
//!     App::new()
//!         .add_plugins(DefaultPlugins)
//!         .add_plugin(IcedPlugin)
//!         .add_event::<UiMessage>()
//!         .add_system(ui_system)
//!         .run();
//! }
//!
//! fn ui_system(time: Res<Time>, mut ctx: IcedContext<UiMessage>) {
//!     ctx.display(text(format!(
//!         "Hello Iced! Running for {:.2} seconds.",
//!         time.elapsed_seconds()
//!     )));
//! }
//! ```
//!
//! ## Feature flags
//!
//! - `touch`: Enables touch input. Is not exclude input from the mouse.

#![deny(unsafe_code)]
#![deny(missing_docs)]

use std::any::{Any, TypeId};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, LockResult, Mutex, MutexGuard
};

use crate::render::{ICED_PASS, IcedNode};
use crate::render::ViewportResource;

use bevy_app::{App, IntoSystemAppConfig, Plugin};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    event::Event,
    prelude::{EventWriter, Query, With},
    system::{NonSendMut, Res, ResMut, Resource, SystemParam}
};
#[cfg(feature = "touch")]
use bevy_input::touch::Touches;
use bevy_math::Vec2;
use bevy_render::{
    main_graph::node::CAMERA_DRIVER,
    render_graph::RenderGraph,
    renderer::{RenderDevice, RenderQueue},
    ExtractSchedule, RenderApp,
};
use bevy_utils::HashMap;
use bevy_window::{CursorIcon, PrimaryWindow, Window};
use iced::{user_interface::Cache as UiCache, UserInterface};
use iced_renderer::{
    graphics::Primitive,
    Backend,
};
pub use iced_runtime as iced;
use iced_runtime::{
    core::{
        clipboard,
        event::Status,
        mouse::Interaction,
        Element, Event as IcedEvent, Point, Size},
    Debug,
};
#[cfg(feature = "touch")]
use iced_runtime::core::touch::Event as TouchEvent;
use iced_style::Theme;
pub use iced_wgpu;
use iced_wgpu::{
    core::{
        renderer::Style,
        Color
    },
    graphics::Viewport,
    wgpu::TextureFormat,
    Backend as WgpuBackend, Settings,
};

mod conversions;
mod render;
mod systems;

use systems::IcedEventQueue;

/// The main feature of `bevy_iced`.
/// Add this to your [`App`] by calling `app.add_plugin(bevy_iced::IcedPlugin)`.
pub struct IcedPlugin {
    settings: Option<Settings>,
}

impl IcedPlugin {
    /// Creates an instance of the plugin with default `iced` settings.
    pub fn default() -> IcedPlugin {
        Self { settings: None }
    }

    /// Creates an instance of the plugin with custom `iced` settings.
    pub fn with_settings(settings: Settings) -> IcedPlugin {
        Self { settings: Some(settings) }
    }
}

impl Plugin for IcedPlugin {
    fn build(&self, app: &mut App) {
        let default_viewport = Viewport::with_physical_size(Size::new(1600, 900), 1.0);
        let default_viewport = ViewportResource(default_viewport);
        let settings = self.settings.unwrap_or(Default::default());
        let iced_resource: IcedResource = IcedProps::new(app, settings).into();

        app.add_system(systems::process_input)
            .add_system(render::update_viewport)
            .insert_resource(DidDraw::default())
            .insert_resource(iced_resource.clone())
            .insert_resource(IcedSettings::default())
            .insert_non_send_resource(IcedCache::default())
            .insert_resource(IcedEventQueue::default())
            .init_resource::<IcedDisplayResult>()
            .insert_resource(default_viewport.clone());

        let render_app = app.sub_app_mut(RenderApp);
        render_app
            .insert_resource(default_viewport)
            .insert_resource(iced_resource)
            .add_system(render::extract_iced_data.in_schedule(ExtractSchedule));
        setup_pipeline(&mut render_app.world.get_resource_mut().unwrap());
    }
}

type Renderer = iced_renderer::Renderer<Theme>;

struct IcedProps {
    renderer: Renderer,
    debug: Debug,
    clipboard: clipboard::Null,
}

impl IcedProps {
    fn new(app: &App, settings: Settings) -> Self {
        let render_world = &app.sub_app(RenderApp).world;
        let device = render_world
            .get_resource::<RenderDevice>()
            .unwrap()
            .wgpu_device();
        let queue = render_world
            .get_resource::<RenderQueue>()
            .unwrap();
        let format = TextureFormat::Bgra8UnormSrgb;

        Self {
            renderer: Renderer::new(Backend::Wgpu(WgpuBackend::new(
                device,
                queue,
                settings,
                format,
            ))),
            debug: Debug::new(),
            clipboard: clipboard::Null,
        }
    }
}

#[derive(Resource, Clone)]
struct IcedResource(Arc<Mutex<IcedProps>>);

impl IcedResource {
    fn lock(&self) -> LockResult<MutexGuard<IcedProps>> {
        self.0.lock()
    }
}

impl From<IcedProps> for IcedResource {
    fn from(value: IcedProps) -> Self {
        Self(Arc::new(Mutex::new(value)))
    }
}

fn setup_pipeline(graph: &mut RenderGraph) {
    graph.add_node(ICED_PASS, IcedNode);
    graph.add_node_edge(CAMERA_DRIVER, ICED_PASS);
}

#[doc(hidden)]
#[derive(Default)]
pub struct IcedCache {
    cache: HashMap<TypeId, Option<UiCache>>,
}

impl IcedCache {
    fn get<M: Any>(&mut self) -> &mut Option<UiCache> {
        let id = TypeId::of::<M>();
        if !self.cache.contains_key(&id) {
            self.cache.insert(id, Some(Default::default()));
        }
        self.cache.get_mut(&id).unwrap()
    }
}

/// Settings used to independently customize Iced rendering.
#[derive(Clone, Resource)]
pub struct IcedSettings {
    /// The scale factor to use for rendering Iced elements.
    /// Setting this to `None` defaults to using the `Window`s scale factor.
    pub scale_factor: Option<f64>,
    /// The theme to use for rendering Iced elements.
    pub theme: Theme,
    /// The style to use for rendering Iced elements.
    pub style: Style,
}

impl IcedSettings {
    /// Set the `scale_factor` used to render Iced elements.
    pub fn set_scale_factor(&mut self, factor: impl Into<Option<f64>>) {
        self.scale_factor = factor.into();
    }
}

impl Default for IcedSettings {
    fn default() -> Self {
        Self {
            scale_factor: None,
            theme: Theme::Dark,
            style: Style {
                text_color: Color::WHITE,
            },
        }
    }
}

/// Result of a [`display`] pass.
#[derive(Default, Resource)]
pub struct IcedDisplayResult {
    /// Contains all events that were captured during the pass.
    pub captured_events: Vec<IcedEvent>,
    /// Is the mouse cursor over some interactive element?
    pub wants_pointer_input: bool,
}

// An atomic flag for updating the draw state.
#[derive(Resource, Deref, DerefMut, Default)]
pub(crate) struct DidDraw(AtomicBool);

/// The context for interacting with Iced. Add this as a parameter to your system.
/// ```no_run
/// fn ui_system(..., mut ctx: IcedContext<UiMessage>) {
///     let element = ...; // Build your element
///     ctx.display(element);
/// }
/// ```
///
/// `IcedContext<T>` requires an event system to be defined in the [`App`].
/// Do so by invoking `app.add_event::<T>()` when constructing your App.
#[derive(SystemParam)]
pub struct IcedContext<'w, 's, Message: Event> {
    viewport: Res<'w, ViewportResource>,
    props: Res<'w, IcedResource>,
    settings: Res<'w, IcedSettings>,
    windows: Query<'w, 's, &'static mut Window, With<PrimaryWindow>>,
    events: ResMut<'w, IcedEventQueue>,
    cache_map: NonSendMut<'w, IcedCache>,
    messages: EventWriter<'w, Message>,
    did_draw: ResMut<'w, DidDraw>,
    #[cfg(feature = "touch")]
    touches: Res<'w, Touches>,
    result: ResMut<'w, IcedDisplayResult>,
}

impl<'w, 's, M: Event> IcedContext<'w, 's, M> {
    /// Display an [`Element`] to the screen.
    pub fn display<'a>(&'a mut self, element: impl Into<Element<'a, M, Renderer>>) {
        let IcedProps {
            ref mut renderer,
            ref mut clipboard,
            ..
        } = &mut *self.props.lock().unwrap();
        let bounds = self.viewport.logical_size();

        let element = element.into();

        let cursor_position = {
            let window = self.windows.single();

            window
                .cursor_position()
                .map(|Vec2 { x, y }| Point {
                    x: x * bounds.width / window.width(),
                    y: (window.height() - y) * bounds.height / window.height(),
                })
                .or_else(|| process_touch_input(self))
                .unwrap_or(Point::ORIGIN)
        };

        let mut messages = Vec::<M>::new();
        let cache_entry = self.cache_map.get::<M>();
        let cache = cache_entry.take().unwrap();
        let mut ui = UserInterface::build(element, bounds, cache, renderer);
        let (_, event_statuses) = ui.update(
            self.events.as_slice(),
            cursor_position,
            renderer,
            clipboard,
            &mut messages,
        );

        messages.into_iter().for_each(|msg| self.messages.send(msg));

        let interaction = ui.draw(
            renderer,
            &self.settings.theme,
            &self.settings.style,
            cursor_position,
        );
        self.windows.single_mut().cursor.icon = match interaction {
            Interaction::Idle => CursorIcon::Default,
            Interaction::Pointer => CursorIcon::Hand,
            Interaction::Grab => CursorIcon::Grab,
            Interaction::Text => CursorIcon::Text,
            Interaction::Crosshair => CursorIcon::Crosshair,
            Interaction::Working => CursorIcon::Progress,
            Interaction::Grabbing => CursorIcon::Grabbing,
            Interaction::ResizingHorizontally => CursorIcon::ColResize,
            Interaction::ResizingVertically => CursorIcon::RowResize,
            Interaction::NotAllowed => CursorIcon::NotAllowed,
        };

        self.result.captured_events = self.events.iter()
            .zip(event_statuses)
            .filter_map(|(ev, status)|
                if status == Status::Captured { Some(ev.clone()) } else { None })
            .collect::<Vec<_>>();
        self.events.clear();
        *cache_entry = Some(ui.into_cache());
        self.did_draw
            .store(true, Ordering::Relaxed);

        let mut wants_pointer_input = false;
        renderer.with_primitives(|_, primitives| primitives.iter().for_each(|primitive| {
            if !wants_pointer_input && hit_test(primitive, cursor_position) {
                wants_pointer_input = true;
            }
        }));
        self.result.wants_pointer_input = wants_pointer_input;
    }
}

fn hit_test(primitive: &Primitive, cursor_position: Point) -> bool {
    match primitive {
        Primitive::Quad { bounds, .. } => bounds.contains(cursor_position),
        Primitive::Text { bounds, .. } => bounds.contains(cursor_position),
        Primitive::Image { bounds, .. } => bounds.contains(cursor_position),
        Primitive::Group { primitives } => primitives.iter().any(|p| hit_test(p, cursor_position)),
        Primitive::Clip { bounds, content } =>
            bounds.contains(cursor_position) && hit_test(content, cursor_position),
        Primitive::Translate { translation, content } =>
            hit_test(content, cursor_position + *translation),
        Primitive::Svg { bounds, .. } => bounds.contains(cursor_position),
        _ => false
    }
}

#[cfg(feature = "touch")]
/// To correctly process input as last resort events are used
fn process_touch_input<M: Event>(context: &IcedContext<M>) -> Option<Point> {
    context
        .touches
        .first_pressed_position()
        .or(context
            .touches
            .iter_just_released()
            .map(|touch| touch.position())
            .next())
        .map(|Vec2 { x, y }| Point { x, y })
        .or(context
            .events
            .iter()
            .filter_map(|ev| {
                if let IcedEvent::Touch(
                    TouchEvent::FingerLifted { position, .. }
                    | TouchEvent::FingerLost { position, .. }
                    | TouchEvent::FingerMoved { position, .. }
                    | TouchEvent::FingerPressed { position, .. },
                ) = ev
                {
                    Some(position)
                } else {
                    None
                }
            })
            .next()
            .copied())
}

#[cfg(not(feature = "touch"))]
fn process_touch_input<M: Event>(_: &IcedContext<M>) -> Option<Point> {
    None
}
