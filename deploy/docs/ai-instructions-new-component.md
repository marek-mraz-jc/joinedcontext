We deploy our platform using **Helmfile** with **Helm charts**.
Every component has its own module (folder) in this repository.
They all follow the same standard and structure:

## Project Structure

- `components/`: All files required for an individual component
  - `<component>/charts.yaml`: version and source of Helm charts
  - `<component>/images.yaml`: version and source of container images
  - `<component>/component.yaml`: High level definition of the component and its individual parts
  - `<component>/default-environment.yaml.gotmpl`: Default environment values for the component
  - `<component>/helmfile.yaml.gotmpl`: Helmfile template definition which renders `component.yaml` and `charts.yaml` into a Helmfile
  - `<component>/values/`: Helm values per component part
  - `<component>/charts/`: Custom Helm charts for the component (if any)
  - `<component>/databases.yaml.gotmpl/`: Required database definitions for the component (if any)
- `defaults/`: Default values for helm, global settings and the collection of component default environments
- `deployment/`: Folder where deployments can be defined (not part of this repository because they are deployment specific)
  - `addons/`: Additional not in the core included components for the deployment
  - `environments/`: Environment specific configuration for every environment
  - `helmfile.yaml`: Entry Helmfile for the deployment
- `docs/`: Documentation
- `template-component/`: Template for new components
- `tests/`: Test cases for components and deployments

## Your Role

You are a developer which writes the config files for a single component.
The structure is already created via a template. You just have to fill the files.

Avoid comments except they show something which is not obvious or a workaround.

## Instructions

Everything you have to do is described in the following list.

1. Configure helm charts in `components/<component>/charts.yaml`
2. Configure values for every release in `components/<component>/values`
    - You can see patterns to use in `docs/developer-guide.md`. You must use them like this, so read them carefully and use the relevant parts for this component. parts are: Connecting to Kafka Cluster, Connecting to Database, Setting Docker Images, Generating Secrets, Exposing UIs and APIs, Connect with Keycloak, Configuring Resources, Setting other values
    - Keep the checklist comment at the top and mark them as checked. Add a comment when they don't need to be set. e.g. `-> not applicable`
    - Keep items unchecked, which are currently not supported.
    - Keep the item common labels unchecked, since we are missing a rule for which labels to set.
    - `base-values.yaml.gotmpl` is to integrate the component in the platform and configure sso, database connections etc.
    - `development.yaml.gotmpl` are values on top of base and is for local development, and it should be lightweight configured
    - `production.yaml.gotmpl` are values on top of base and is for production deployments with security and availability features enabled.
3. Update the README.md of the component. Include a description, required (external) components, (optional) other requirements, a configuration table with values from the default environment
4. Update component to `defaults/environment/global.yaml` - `components` list
