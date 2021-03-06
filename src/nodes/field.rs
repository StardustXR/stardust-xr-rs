use super::core::Node;
use super::spatial::{get_spatial_parent_flex, Spatial};
use crate::core::client::Client;
use anyhow::{anyhow, ensure, Result};
use glam::{swizzles::*, vec2, vec3, vec3a, Mat4, Vec3, Vec3A};
use libstardustxr::fusion::flex::FlexBuffable;
use libstardustxr::{flex_to_quat, flex_to_vec3};
use parking_lot::Mutex;
use portable_atomic::AtomicF32;
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub trait FieldTrait {
	fn local_distance(&self, p: Vec3A) -> f32;
	fn local_normal(&self, p: Vec3A, r: f32) -> Vec3A {
		let d = self.local_distance(p);
		let e = vec2(r, 0_f32);

		let n = vec3a(d, d, d)
			- vec3a(
				self.local_distance(vec3a(e.x, e.y, e.y)),
				self.local_distance(vec3a(e.y, e.x, e.y)),
				self.local_distance(vec3a(e.y, e.y, e.x)),
			);

		n.normalize()
	}
	fn local_closest_point(&self, p: Vec3A, r: f32) -> Vec3A {
		p - (self.local_normal(p, r) * self.local_distance(p))
	}

	fn distance(&self, reference_space: &Spatial, p: Vec3A) -> f32 {
		let reference_to_local_space =
			Spatial::space_to_space_matrix(Some(reference_space), Some(self.spatial_ref()));
		let local_p = reference_to_local_space.transform_point3a(p);
		self.local_distance(local_p)
	}
	fn normal(&self, reference_space: &Spatial, p: Vec3A, r: f32) -> Vec3A {
		let reference_to_local_space =
			Spatial::space_to_space_matrix(Some(reference_space), Some(self.spatial_ref()));
		let local_p = reference_to_local_space.transform_point3a(p);
		reference_to_local_space
			.inverse()
			.transform_vector3a(self.local_normal(local_p, r))
	}
	fn closest_point(&self, reference_space: &Spatial, p: Vec3A, r: f32) -> Vec3A {
		let reference_to_local_space =
			Spatial::space_to_space_matrix(Some(reference_space), Some(self.spatial_ref()));
		let local_p = reference_to_local_space.transform_point3a(p);
		reference_to_local_space
			.inverse()
			.transform_point3a(self.local_closest_point(local_p, r))
	}

	fn add_field_methods(&self, node: &Arc<Node>) {
		node.add_local_method("distance", field_distance_flex);
		node.add_local_method("normal", field_normal_flex);
		node.add_local_method("closest_point", field_closest_point_flex);
	}

	fn spatial_ref(&self) -> &Spatial;
}

fn field_distance_flex(node: &Node, calling_client: Arc<Client>, data: &[u8]) -> Result<Vec<u8>> {
	let flex_vec = flexbuffers::Reader::get_root(data)?.get_vector()?;
	let reference_space = calling_client
		.scenegraph
		.get_node(flex_vec.idx(0).as_str())
		.ok_or_else(|| anyhow!("Reference space node does not exist"))?
		.spatial
		.get()
		.ok_or_else(|| anyhow!("Reference space node does not have a spatial"))?
		.clone();
	let point = flex_to_vec3!(flex_vec.idx(1)).ok_or_else(|| anyhow!("Point is invalid"))?;

	let distance = node
		.field
		.get()
		.unwrap()
		.distance(reference_space.as_ref(), point.into());
	Ok(FlexBuffable::from(distance).build_singleton())
}
fn field_normal_flex(node: &Node, calling_client: Arc<Client>, data: &[u8]) -> Result<Vec<u8>> {
	let flex_vec = flexbuffers::Reader::get_root(data)?.get_vector()?;
	let reference_space = calling_client
		.scenegraph
		.get_node(flex_vec.idx(0).as_str())
		.ok_or_else(|| anyhow!("Reference space node does not exist"))?
		.spatial
		.get()
		.ok_or_else(|| anyhow!("Reference space node does not have a spatial"))?
		.clone();
	let point = flex_to_vec3!(flex_vec.idx(1)).ok_or_else(|| anyhow!("Point is invalid"))?;

	let normal = node.field.get().as_ref().unwrap().normal(
		reference_space.as_ref(),
		point.into(),
		0.001_f32,
	);
	Ok(FlexBuffable::from(mint::Vector3::from(normal)).build_singleton())
}
fn field_closest_point_flex(
	node: &Node,
	calling_client: Arc<Client>,
	data: &[u8],
) -> Result<Vec<u8>> {
	let flex_vec = flexbuffers::Reader::get_root(data)?.get_vector()?;
	let reference_space = calling_client
		.scenegraph
		.get_node(flex_vec.idx(0).as_str())
		.ok_or_else(|| anyhow!("Reference space node does not exist"))?
		.spatial
		.get()
		.ok_or_else(|| anyhow!("Reference space node does not have a spatial"))?
		.clone();
	let point = flex_to_vec3!(flex_vec.idx(1)).ok_or_else(|| anyhow!("Point is invalid"))?;

	let closest_point =
		node.field
			.get()
			.unwrap()
			.closest_point(reference_space.as_ref(), point.into(), 0.001_f32);
	Ok(FlexBuffable::from(mint::Vector3::from(closest_point)).build_singleton())
}

pub enum Field {
	Box(BoxField),
	Cylinder(CylinderField),
	Sphere(SphereField),
}

impl Deref for Field {
	type Target = dyn FieldTrait;
	fn deref(&self) -> &Self::Target {
		match self {
			Field::Box(field) => field,
			Field::Cylinder(field) => field,
			Field::Sphere(field) => field,
		}
	}
}

pub struct BoxField {
	space: Arc<Spatial>,
	size: Mutex<Vec3>,
}

impl BoxField {
	pub fn add_to(node: &Arc<Node>, size: Vec3) -> Result<()> {
		ensure!(
			node.spatial.get().is_some(),
			"Internal: Node does not have a spatial attached!"
		);
		ensure!(
			node.field.get().is_none(),
			"Internal: Node already has a field attached!"
		);
		let box_field = BoxField {
			space: node.spatial.get().unwrap().clone(),
			size: Mutex::new(size),
		};
		box_field.add_field_methods(node);
		node.add_local_signal("setSize", BoxField::set_size_flex);
		let _ = node.field.set(Arc::new(Field::Box(box_field)));
		Ok(())
	}

	pub fn set_size(&self, size: Vec3) {
		*self.size.lock() = size;
	}

	pub fn set_size_flex(node: &Node, _calling_client: Arc<Client>, data: &[u8]) -> Result<()> {
		let root = flexbuffers::Reader::get_root(data)?;
		let size = flex_to_vec3!(root).ok_or_else(|| anyhow!("Size is invalid"))?;
		if let Field::Box(box_field) = node.field.get().unwrap().as_ref() {
			box_field.set_size(size.into());
		}
		Ok(())
	}
}

impl FieldTrait for BoxField {
	fn local_distance(&self, p: Vec3A) -> f32 {
		let size = self.size.lock();
		let q = vec3(
			p.x.abs() - (size.x * 0.5_f32),
			p.y.abs() - (size.y * 0.5_f32),
			p.z.abs() - (size.z * 0.5_f32),
		);
		let v = vec3a(q.x.max(0_f32), q.y.max(0_f32), q.z.max(0_f32));
		v.length() + q.x.max(q.y.max(q.z)).min(0_f32)
	}
	fn spatial_ref(&self) -> &Spatial {
		self.space.as_ref()
	}
}

pub struct CylinderField {
	space: Arc<Spatial>,
	length: AtomicF32,
	radius: AtomicF32,
}

impl CylinderField {
	pub fn add_to(node: &Arc<Node>, length: f32, radius: f32) -> Result<()> {
		ensure!(
			node.spatial.get().is_some(),
			"Internal: Node does not have a spatial attached!"
		);
		ensure!(
			node.field.get().is_none(),
			"Internal: Node already has a field attached!"
		);
		let cylinder_field = CylinderField {
			space: node.spatial.get().unwrap().clone(),
			length: AtomicF32::new(length),
			radius: AtomicF32::new(radius),
		};
		cylinder_field.add_field_methods(node);
		node.add_local_signal("setSize", CylinderField::set_size_flex);
		let _ = node.field.set(Arc::new(Field::Cylinder(cylinder_field)));
		Ok(())
	}

	pub fn set_size(&self, length: f32, radius: f32) {
		self.length.store(length, Ordering::Relaxed);
		self.radius.store(radius, Ordering::Relaxed);
	}

	pub fn set_size_flex(node: &Node, _calling_client: Arc<Client>, data: &[u8]) -> Result<()> {
		let flex_vec = flexbuffers::Reader::get_root(data)?.get_vector()?;
		let length = flex_vec.idx(0).as_f32();
		let radius = flex_vec.idx(1).as_f32();
		if let Field::Cylinder(cylinder_field) = node.field.get().unwrap().as_ref() {
			cylinder_field.set_size(length, radius);
		}
		Ok(())
	}
}

impl FieldTrait for CylinderField {
	fn local_distance(&self, p: Vec3A) -> f32 {
		let radius = self.length.load(Ordering::Relaxed);
		let d = vec2(p.xy().length().abs() - radius, p.z.abs() - (radius * 0.5));

		d.x.max(d.y).min(0_f32)
			+ (if d.x >= 0_f32 && d.y >= 0_f32 {
				d.length()
			} else {
				0_f32
			})
	}
	fn spatial_ref(&self) -> &Spatial {
		self.space.as_ref()
	}
}

pub struct SphereField {
	space: Arc<Spatial>,
	radius: AtomicF32,
}

impl SphereField {
	pub fn add_to(node: &Arc<Node>, radius: f32) -> Result<()> {
		ensure!(
			node.spatial.get().is_some(),
			"Internal: Node does not have a spatial attached!"
		);
		ensure!(
			node.field.get().is_none(),
			"Internal: Node already has a field attached!"
		);
		let sphere_field = SphereField {
			space: node.spatial.get().unwrap().clone(),
			radius: AtomicF32::new(radius),
		};
		sphere_field.add_field_methods(node);
		node.add_local_signal("setRadius", SphereField::set_radius_flex);
		let _ = node.field.set(Arc::new(Field::Sphere(sphere_field)));
		Ok(())
	}

	pub fn set_radius(&self, radius: f32) {
		self.radius.store(radius, Ordering::Relaxed);
	}

	pub fn set_radius_flex(node: &Node, _calling_client: Arc<Client>, data: &[u8]) -> Result<()> {
		let root = flexbuffers::Reader::get_root(data)?;
		if let Field::Sphere(sphere_field) = node.field.get().unwrap().as_ref() {
			sphere_field.set_radius(root.as_f32());
		}
		Ok(())
	}
}

impl FieldTrait for SphereField {
	fn local_distance(&self, p: Vec3A) -> f32 {
		p.length() - self.radius.load(Ordering::Relaxed)
	}
	fn local_normal(&self, p: Vec3A, _r: f32) -> Vec3A {
		-p.normalize()
	}
	fn local_closest_point(&self, p: Vec3A, _r: f32) -> Vec3A {
		p.normalize() * self.radius.load(Ordering::Relaxed)
	}
	fn spatial_ref(&self) -> &Spatial {
		self.space.as_ref()
	}
}

pub fn create_interface(client: &Arc<Client>) {
	let node = Node::create(client, "", "field", false);
	node.add_local_signal("createBoxField", create_box_field_flex);
	node.add_local_signal("createCylinderField", create_cylinder_field_flex);
	node.add_local_signal("createSphereField", create_sphere_field_flex);
	node.add_to_scenegraph();
}

pub fn create_box_field_flex(_node: &Node, calling_client: Arc<Client>, data: &[u8]) -> Result<()> {
	let flex_vec = flexbuffers::Reader::get_root(data)?.get_vector()?;
	let node = Node::create(&calling_client, "/field", flex_vec.idx(0).get_str()?, true);
	let parent = get_spatial_parent_flex(&calling_client, flex_vec.idx(1).get_str()?)?;
	let transform = Mat4::from_rotation_translation(
		flex_to_quat!(flex_vec.idx(3))
			.ok_or_else(|| anyhow!("Rotation not found"))?
			.into(),
		flex_to_vec3!(flex_vec.idx(2))
			.ok_or_else(|| anyhow!("Position not found"))?
			.into(),
	);
	let size = flex_to_vec3!(flex_vec.idx(4)).ok_or_else(|| anyhow!("Size invalid"))?;
	let node = node.add_to_scenegraph();
	Spatial::add_to(&node, Some(parent), transform)?;
	BoxField::add_to(&node, size.into())?;
	Ok(())
}

pub fn create_cylinder_field_flex(
	_node: &Node,
	calling_client: Arc<Client>,
	data: &[u8],
) -> Result<()> {
	let flex_vec = flexbuffers::Reader::get_root(data)?.get_vector()?;
	let node = Node::create(&calling_client, "/field", flex_vec.idx(0).get_str()?, true);
	let parent = get_spatial_parent_flex(&calling_client, flex_vec.idx(1).get_str()?)?;
	let transform = Mat4::from_rotation_translation(
		flex_to_quat!(flex_vec.idx(3))
			.ok_or_else(|| anyhow!("Rotation not found"))?
			.into(),
		flex_to_vec3!(flex_vec.idx(2))
			.ok_or_else(|| anyhow!("Position not found"))?
			.into(),
	);
	let length = flex_vec.idx(0).as_f32();
	let radius = flex_vec.idx(1).as_f32();
	let node = node.add_to_scenegraph();
	Spatial::add_to(&node, Some(parent), transform)?;
	CylinderField::add_to(&node, length, radius)?;
	Ok(())
}

pub fn create_sphere_field_flex(
	_node: &Node,
	calling_client: Arc<Client>,
	data: &[u8],
) -> Result<()> {
	let flex_vec = flexbuffers::Reader::get_root(data)?.get_vector()?;
	let node = Node::create(&calling_client, "/field", flex_vec.idx(0).get_str()?, true);
	let parent = get_spatial_parent_flex(&calling_client, flex_vec.idx(1).get_str()?)?;
	let transform = Mat4::from_translation(
		flex_to_vec3!(flex_vec.idx(2))
			.ok_or_else(|| anyhow!("Position not found"))?
			.into(),
	);
	let node = node.add_to_scenegraph();
	Spatial::add_to(&node, Some(parent), transform)?;
	SphereField::add_to(&node, flex_vec.idx(3).as_f32())?;
	Ok(())
}

pub struct Ray {
	pub origin: Vec3,
	pub direction: Vec3,
	pub space: Arc<Spatial>,
}

pub struct RayMarchResult {
	pub ray: Ray,
	pub distance: f32,
	pub deepest_point_distance: f32,
	pub ray_length: f32,
	pub ray_steps: u32,
}

// const MIN_RAY_STEPS: u32 = 0;
const MAX_RAY_STEPS: u32 = 1000;

const MIN_RAY_MARCH: f32 = 0.001_f32;
const MAX_RAY_MARCH: f32 = f32::MAX;

// const MIN_RAY_LENGTH: f32 = 0_f32;
const MAX_RAY_LENGTH: f32 = 1000_f32;

pub fn ray_march(ray: Ray, field: &Field) -> RayMarchResult {
	let mut result = RayMarchResult {
		ray,
		distance: f32::MAX,
		deepest_point_distance: 0_f32,
		ray_length: 0_f32,
		ray_steps: 0,
	};

	let ray_to_field_matrix =
		Spatial::space_to_space_matrix(Some(&result.ray.space), Some(field.spatial_ref()));
	let mut ray_point = ray_to_field_matrix.transform_point3a(result.ray.origin.into());
	let ray_direction = ray_to_field_matrix.transform_vector3a(result.ray.direction.into());

	while result.ray_steps < MAX_RAY_STEPS && result.ray_length < MAX_RAY_LENGTH {
		let distance = field.local_distance(ray_point);
		let march_distance = distance.clamp(MIN_RAY_MARCH, MAX_RAY_MARCH);

		result.ray_length += march_distance;
		ray_point += ray_direction * march_distance;

		if result.distance > distance {
			result.deepest_point_distance = result.ray_length;
		}
		result.distance = distance.min(result.distance);

		result.ray_steps += 1;
	}

	result
}
